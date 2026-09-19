use std::collections::HashSet;
use std::future::Future;
use std::num::NonZeroUsize;
use std::task::Poll;

use mc_script::{ScriptEvent, ScriptPlayerId, script_boundary_pair};
use tokio::sync::mpsc;

use super::SessionRegistry;
use super::outbound::OutboundCommand;
use super::script_menu_endpoint::{
    ScriptMenuCloseRequest, ScriptMenuRouteError, publish_script_menu_click,
};
use crate::login::LoggedInProfile;
use crate::play::PlayerPose;
use crate::server::ScriptEventSink;

fn profile(name: &str) -> LoggedInProfile {
    LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_owned(),
    }
}

#[tokio::test]
async fn close_routes_to_the_exact_connected_session() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(1);
    let (session_id, _) = registry.register(
        &profile("ScriptMenuOwner"),
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let request = ScriptMenuCloseRequest {
        plugin_id: "catalog".to_owned(),
        player_id: ScriptPlayerId::new(session_id),
        menu_id: "main".to_owned(),
    };

    registry
        .dispatch_script_menu_close_for_test(request.clone())
        .unwrap();

    match rx.recv().await.unwrap() {
        OutboundCommand::CloseScriptMenu(actual) => assert_eq!(actual, request),
        other => panic!("expected script menu close, got {other:?}"),
    }
}

#[tokio::test]
async fn close_waits_behind_existing_reliable_session_pressure() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(1);
    tx.try_send(OutboundCommand::SystemChat {
        message: "first".to_owned(),
    })
    .unwrap();
    let (session_id, _) = registry.register(
        &profile("MenuPressure"),
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let request = ScriptMenuCloseRequest {
        plugin_id: "catalog".to_owned(),
        player_id: ScriptPlayerId::new(session_id),
        menu_id: "main".to_owned(),
    };

    registry
        .dispatch_script_menu_close_for_test(request.clone())
        .unwrap();

    assert!(matches!(
        rx.recv().await.unwrap(),
        OutboundCommand::SystemChat { message } if message == "first"
    ));
    match rx.recv().await.unwrap() {
        OutboundCommand::CloseScriptMenu(actual) => assert_eq!(actual, request),
        other => panic!("expected pressured script menu close, got {other:?}"),
    }
}

#[test]
fn disconnected_close_is_rejected_without_an_outbound_command() {
    let registry = SessionRegistry::new();
    let result = registry.dispatch_script_menu_close_for_test(ScriptMenuCloseRequest {
        plugin_id: "catalog".to_owned(),
        player_id: ScriptPlayerId::new(77),
        menu_id: "main".to_owned(),
    });

    assert_eq!(result, Err(ScriptMenuRouteError::PlayerDisconnected));
}

#[test]
fn registered_session_with_closed_outbound_lane_is_rejected() {
    let registry = SessionRegistry::new();
    let (tx, rx) = mpsc::channel(1);
    let (session_id, _) = registry.register(
        &profile("ClosedMenuLane"),
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    drop(rx);

    let result = registry.dispatch_script_menu_close_for_test(ScriptMenuCloseRequest {
        plugin_id: "catalog".to_owned(),
        player_id: ScriptPlayerId::new(session_id),
        menu_id: "main".to_owned(),
    });

    assert_eq!(result, Err(ScriptMenuRouteError::PlayerDisconnected));
}

#[tokio::test]
async fn targeted_click_waits_for_bounded_queue_capacity_without_dropping() {
    let one = NonZeroUsize::new(1).unwrap();
    let (boundary, mut host) = script_boundary_pair(one, one);
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let sink = ScriptEventSink::new(boundary);
    let mut delivery = Box::pin(publish_script_menu_click(
        Some(&sink),
        ScriptEvent::server_tick(9),
    ));

    std::future::poll_fn(|context| {
        assert!(delivery.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    assert_eq!(
        host.recv_event().await.unwrap().event_name(),
        "server.started"
    );
    assert!(delivery.await);
    assert_eq!(host.recv_event().await.unwrap().event_name(), "server.tick");
}

#[tokio::test]
async fn click_is_rejected_when_the_script_event_sink_is_unavailable() {
    assert!(
        !publish_script_menu_click(None, ScriptEvent::server_tick(9)).await,
        "an unavailable target must not report event delivery"
    );
}

#[tokio::test]
async fn click_is_rejected_when_the_targeted_event_queue_is_closed() {
    let one = NonZeroUsize::new(1).unwrap();
    let (boundary, host) = script_boundary_pair(one, one);
    drop(host);
    let sink = ScriptEventSink::new(boundary);

    assert!(!publish_script_menu_click(Some(&sink), ScriptEvent::server_tick(9)).await);
}
