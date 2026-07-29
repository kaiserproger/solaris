use std::num::NonZeroUsize;

use mc_script::{
    AdmittedScriptCommand, LuaHostConfig, ScriptEvent, ScriptEventKind, ScriptVillagerGoal,
    ScriptVillagerGoalFailure, start_lua_host,
};

use super::villager::{PluginVillagerAdapter, VillagerAdapterError};
use super::{ScriptRouter, ScriptRouterExit};
use crate::server::ScriptEventSink;

async fn admitted_villager_command(plugin_id: &str, command: &str) -> AdmittedScriptCommand {
    let plugins = tempfile::tempdir().unwrap();
    let plugin = plugins.path().join(plugin_id);
    std::fs::create_dir(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        format!(
            r#"id = "{plugin_id}"
name = "Villager test"
version = "0.1.0"
api = "0.6.0"
events = ["server.started"]
capabilities = ["villagers"]
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

fn adapter() -> (PluginVillagerAdapter, mc_script::ScriptHostEndpoint) {
    let (boundary, endpoint) = mc_script::script_boundary_pair(
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    (
        PluginVillagerAdapter::new(ScriptEventSink::new(boundary)),
        endpoint,
    )
}

#[tokio::test]
async fn binding_without_nearby_villager_returns_typed_not_found() {
    let admitted = admitted_villager_command(
        "settlement",
        r#"solaris.bind_nearest_villager("bind", 0, 64, 0, 16)"#,
    )
    .await;
    let (adapter, mut events) = adapter();
    let sessions = crate::play::SessionRegistry::new();

    assert!(
        !adapter
            .route_binding_admitted(admitted, &sessions)
            .await
            .unwrap()
            .accepted
    );
    let event = events.recv_event().await.unwrap();
    assert_eq!(event.target_plugin_id(), Some("settlement"));
    assert!(matches!(
        event.kind(),
        ScriptEventKind::VillagerBindingResult {
            request_id,
            binding: None,
            failure: Some(mc_script::ScriptVillagerBindingFailure::NotFound),
        } if request_id == "bind"
    ));
}

#[tokio::test]
async fn owner_can_move_and_idle_bound_villager_but_foreign_plugin_cannot_use_lease() {
    let owner_binding = admitted_villager_command(
        "owner",
        r#"solaris.bind_nearest_villager("bind", 0, 64, 0, 16)"#,
    )
    .await;
    let (adapter, mut events) = adapter();
    let sessions = crate::play::SessionRegistry::new();
    let villager = sessions.spawn_script_villager_for_test(mc_entity::Vec3::new(3.0, 64.0, 0.0));

    assert!(
        adapter
            .route_binding_admitted(owner_binding, &sessions)
            .await
            .unwrap()
            .accepted
    );
    let lease_id = match events.recv_event().await.unwrap().kind() {
        ScriptEventKind::VillagerBindingResult {
            binding: Some(binding),
            failure: None,
            ..
        } => binding.token().to_owned(),
        other => panic!("unexpected binding result: {other:?}"),
    };
    assert_eq!(
        adapter.binding_owner_for_test(&lease_id).as_deref(),
        Some("owner")
    );

    let foreign = admitted_villager_command(
        "foreign",
        &format!("solaris.set_villager_idle('foreign-idle', '{lease_id}')"),
    )
    .await;
    assert!(
        !adapter
            .route_goal_admitted(foreign, &sessions)
            .await
            .unwrap()
            .accepted
    );
    assert!(matches!(
        events.recv_event().await.unwrap().kind(),
        ScriptEventKind::VillagerGoalResult {
            request_id,
            goal: ScriptVillagerGoal::Idle,
            failure: Some(ScriptVillagerGoalFailure::BindingUnavailable),
        } if request_id == "foreign-idle"
    ));
    assert_eq!(
        sessions.script_entity_goal_for_test(villager),
        Some(mc_entity::GoalState::Idle)
    );

    let moving = admitted_villager_command(
        "owner",
        &format!("solaris.move_villager_to('move-home', '{lease_id}', 8, 64, 2, 0.3)"),
    )
    .await;
    assert!(
        adapter
            .route_goal_admitted(moving, &sessions)
            .await
            .unwrap()
            .accepted
    );
    assert!(matches!(
        events.recv_event().await.unwrap().kind(),
        ScriptEventKind::VillagerGoalResult {
            request_id,
            goal: ScriptVillagerGoal::FollowPosition { .. },
            failure: None,
        } if request_id == "move-home"
    ));
    assert_eq!(
        sessions.script_entity_goal_for_test(villager),
        Some(mc_entity::GoalState::FollowPosition {
            target: mc_entity::Vec3::new(8.0, 64.0, 2.0),
            speed: 0.3,
        })
    );

    let idle = admitted_villager_command(
        "owner",
        &format!("solaris.set_villager_idle('idle', '{lease_id}')"),
    )
    .await;
    assert!(
        adapter
            .route_goal_admitted(idle, &sessions)
            .await
            .unwrap()
            .accepted
    );
    assert!(matches!(
        events.recv_event().await.unwrap().kind(),
        ScriptEventKind::VillagerGoalResult {
            request_id,
            goal: ScriptVillagerGoal::Idle,
            failure: None,
        } if request_id == "idle"
    ));
    assert_eq!(
        sessions.script_entity_goal_for_test(villager),
        Some(mc_entity::GoalState::Idle)
    );
}

#[tokio::test]
async fn router_stops_when_targeted_villager_result_cannot_be_published() {
    let admitted = admitted_villager_command(
        "settlement",
        r#"solaris.bind_nearest_villager("bind", 0, 64, 0, 16)"#,
    )
    .await;
    let (boundary, events) = mc_script::script_boundary_pair(
        NonZeroUsize::new(8).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    drop(events);
    let router = ScriptRouter::new(ScriptEventSink::new(boundary), None);
    let sessions = crate::play::SessionRegistry::new();

    assert_eq!(
        router.route_villager_admitted(admitted, &sessions).await,
        ScriptRouterExit::Stop
    );
}

#[tokio::test]
async fn wrong_admitted_command_is_rejected_without_result() {
    let admitted =
        admitted_villager_command("settlement", r#"solaris.broadcast("not a goal")"#).await;
    let (adapter, mut events) = adapter();
    let sessions = crate::play::SessionRegistry::new();

    assert_eq!(
        adapter.route_goal_admitted(admitted, &sessions).await,
        Err(VillagerAdapterError::WrongCommand)
    );
    drop(adapter);
    assert!(events.recv_event().await.is_none());
}
