use std::num::NonZeroUsize;
use std::sync::Arc;

use mc_script::{
    AdmittedScriptCommand, LuaHostConfig, ScriptEvent, ScriptEventKind, start_lua_host,
};

use super::colony::{ColonyAdapterError, ColonyLimits, PluginColonyAdapter};
use super::{ScriptRouter, ScriptRouterExit};
use crate::server::ScriptEventSink;

async fn admitted_colony_command(plugin_id: &str, command: &str) -> AdmittedScriptCommand {
    let plugins = tempfile::tempdir().unwrap();
    let plugin = plugins.path().join(plugin_id);
    std::fs::create_dir(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        format!(
            r#"id = "{plugin_id}"
name = "Colony test"
version = "0.1.0"
api = "0.6.0"
events = ["server.started"]
capabilities = ["colonies"]
"#,
        ),
    )
    .unwrap();
    std::fs::write(
        plugin.join("main.lua"),
        format!(
            r#"
function on_server_started(_event)
    {command}
end
"#,
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

fn adapter() -> (PluginColonyAdapter, mc_script::ScriptHostEndpoint) {
    let (boundary, endpoint) = mc_script::script_boundary_pair(
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    (
        PluginColonyAdapter::with_limits_for_test(
            ScriptEventSink::new(boundary),
            ColonyLimits {
                total_colonies: 8,
                colonies_per_plugin: 4,
            },
        ),
        endpoint,
    )
}

#[tokio::test]
async fn admitted_colony_upsert_is_owner_scoped_and_publishes_result() {
    let admitted = admitted_colony_command(
        "owner-a",
        r#"solaris.upsert_colony("register", "starter", "Starter", "minecraft:overworld", 1, 64, 2)"#,
    )
    .await;
    let (adapter, mut events) = adapter();

    assert!(adapter.route_admitted(admitted).await.unwrap().accepted);
    let record = adapter
        .record_for_test("owner-a", "starter")
        .expect("owner colony record");
    assert_eq!(record.name(), "Starter");
    assert!(adapter.record_for_test("owner-b", "starter").is_none());

    let event = events.recv_event().await.unwrap();
    assert_eq!(event.target_plugin_id(), Some("owner-a"));
    assert!(matches!(
        event.kind(),
        ScriptEventKind::ColonyRecordResult {
            request_id,
            colony_id,
            accepted: true,
        } if request_id == "register" && colony_id == "starter"
    ));
}

#[tokio::test]
async fn colony_capacity_rejects_new_record_but_allows_replacement() {
    let first = admitted_colony_command(
        "owner",
        r#"solaris.upsert_colony("first", "starter", "Starter", "minecraft:overworld", 1, 64, 2)"#,
    )
    .await;
    let rejected = admitted_colony_command(
        "owner",
        r#"solaris.upsert_colony("second", "other", "Other", "minecraft:overworld", 3, 64, 4)"#,
    )
    .await;
    let replacement = admitted_colony_command(
        "owner",
        r#"solaris.upsert_colony("replace", "starter", "Renamed", "minecraft:overworld", 5, 65, 6)"#,
    )
    .await;
    let (boundary, mut events) = mc_script::script_boundary_pair(
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    let adapter = PluginColonyAdapter::with_limits_for_test(
        ScriptEventSink::new(boundary),
        ColonyLimits {
            total_colonies: 1,
            colonies_per_plugin: 1,
        },
    );

    assert!(adapter.route_admitted(first).await.unwrap().accepted);
    assert!(!adapter.route_admitted(rejected).await.unwrap().accepted);
    assert!(adapter.route_admitted(replacement).await.unwrap().accepted);
    assert!(adapter.record_for_test("owner", "other").is_none());
    assert_eq!(
        adapter.record_for_test("owner", "starter").unwrap().name(),
        "Renamed"
    );

    let first_result = events.recv_event().await.unwrap();
    let rejected_result = events.recv_event().await.unwrap();
    let replacement_result = events.recv_event().await.unwrap();
    assert!(matches!(
        first_result.kind(),
        ScriptEventKind::ColonyRecordResult { accepted: true, .. }
    ));
    assert!(matches!(
        rejected_result.kind(),
        ScriptEventKind::ColonyRecordResult {
            request_id,
            accepted: false,
            ..
        } if request_id == "second"
    ));
    assert!(matches!(
        replacement_result.kind(),
        ScriptEventKind::ColonyRecordResult {
            request_id,
            accepted: true,
            ..
        } if request_id == "replace"
    ));
}

#[tokio::test]
async fn per_plugin_colony_capacity_does_not_block_another_owner() {
    let owner_first = admitted_colony_command(
        "owner",
        r#"solaris.upsert_colony("first", "first", "First", "minecraft:overworld", 0, 64, 0)"#,
    )
    .await;
    let owner_second = admitted_colony_command(
        "owner",
        r#"solaris.upsert_colony("second", "second", "Second", "minecraft:overworld", 0, 64, 0)"#,
    )
    .await;
    let other = admitted_colony_command(
        "other",
        r#"solaris.upsert_colony("other", "first", "Other", "minecraft:overworld", 0, 64, 0)"#,
    )
    .await;
    let (boundary, _events) = mc_script::script_boundary_pair(
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    let adapter = PluginColonyAdapter::with_limits_for_test(
        ScriptEventSink::new(boundary),
        ColonyLimits {
            total_colonies: 2,
            colonies_per_plugin: 1,
        },
    );

    assert!(adapter.route_admitted(owner_first).await.unwrap().accepted);
    assert!(!adapter.route_admitted(owner_second).await.unwrap().accepted);
    assert!(adapter.route_admitted(other).await.unwrap().accepted);
}

#[tokio::test]
async fn wrong_admitted_command_is_rejected_without_state_or_result() {
    let admitted = admitted_colony_command("owner", r#"solaris.broadcast("not a colony")"#).await;
    let (adapter, mut events) = adapter();

    assert_eq!(
        adapter.route_admitted(admitted).await,
        Err(ColonyAdapterError::WrongCommand)
    );
    assert!(adapter.record_for_test("owner", "starter").is_none());
    drop(adapter);
    assert!(events.recv_event().await.is_none());
}

#[tokio::test]
async fn closed_result_queue_reports_failure_after_committed_upsert() {
    let admitted = admitted_colony_command(
        "owner",
        r#"solaris.upsert_colony("register", "starter", "Starter", "minecraft:overworld", 0, 64, 0)"#,
    )
    .await;
    let (adapter, events) = adapter();
    drop(events);

    assert_eq!(
        adapter.route_admitted(admitted).await,
        Err(ColonyAdapterError::PublicationClosed)
    );
    assert!(adapter.record_for_test("owner", "starter").is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_capacity_contenders_commit_exactly_one_record() {
    let first = admitted_colony_command(
        "owner",
        r#"solaris.upsert_colony("first", "first", "First", "minecraft:overworld", 0, 64, 0)"#,
    )
    .await;
    let second = admitted_colony_command(
        "owner",
        r#"solaris.upsert_colony("second", "second", "Second", "minecraft:overworld", 0, 64, 0)"#,
    )
    .await;
    let (boundary, mut events) = mc_script::script_boundary_pair(
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    let adapter = PluginColonyAdapter::with_limits_for_test(
        ScriptEventSink::new(boundary),
        ColonyLimits {
            total_colonies: 1,
            colonies_per_plugin: 1,
        },
    );

    let ready = Arc::new(tokio::sync::Barrier::new(3));
    let first_task = {
        let adapter = adapter.clone();
        let ready = Arc::clone(&ready);
        tokio::spawn(async move {
            ready.wait().await;
            adapter.route_admitted(first).await
        })
    };
    let second_task = {
        let adapter = adapter.clone();
        let ready = Arc::clone(&ready);
        tokio::spawn(async move {
            ready.wait().await;
            adapter.route_admitted(second).await
        })
    };
    ready.wait().await;
    let first_outcome = first_task.await.unwrap().unwrap();
    let second_outcome = second_task.await.unwrap().unwrap();
    let accepted = usize::from(first_outcome.accepted) + usize::from(second_outcome.accepted);
    assert_eq!(accepted, 1);
    assert_ne!(
        adapter.record_for_test("owner", "first").is_some(),
        adapter.record_for_test("owner", "second").is_some()
    );
    let first_result = events.recv_event().await.unwrap();
    let second_result = events.recv_event().await.unwrap();
    let published_accepted = [first_result, second_result]
        .into_iter()
        .filter(|event| {
            matches!(
                event.kind(),
                ScriptEventKind::ColonyRecordResult { accepted: true, .. }
            )
        })
        .count();
    assert_eq!(published_accepted, 1);
}

#[tokio::test]
async fn router_stops_when_colony_result_publication_is_closed() {
    let admitted = admitted_colony_command(
        "owner",
        r#"solaris.upsert_colony("register", "starter", "Starter", "minecraft:overworld", 0, 64, 0)"#,
    )
    .await;
    let (boundary, events) = mc_script::script_boundary_pair(
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    drop(events);
    let router = ScriptRouter::new(ScriptEventSink::new(boundary), None);

    assert_eq!(
        router.route_colony_admitted(admitted).await,
        ScriptRouterExit::Stop
    );
}

#[tokio::test]
async fn router_keeps_villager_binding_fail_closed_without_a_result() {
    let admitted = admitted_colony_command(
        "owner",
        r#"solaris.bind_nearest_villager("bind", "starter", 0, 64, 0, 16)"#,
    )
    .await;
    let (boundary, mut events) = mc_script::script_boundary_pair(
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    let router = ScriptRouter::new(ScriptEventSink::new(boundary), None);

    assert_eq!(
        router.route_colony_admitted(admitted).await,
        ScriptRouterExit::Continue
    );
    drop(router);
    assert!(events.recv_event().await.is_none());
}
