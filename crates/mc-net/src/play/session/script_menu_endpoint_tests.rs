use std::collections::HashSet;
use std::future::Future;
use std::num::NonZeroUsize;
use std::task::Poll;

use mc_data::items::solaris_required_items;
use mc_protocol::packets::play::ContainerInput;
use mc_script::{
    LuaHostConfig, ScriptEvent, ScriptEventKind, ScriptInventoryClick, ScriptPlayerContext,
    ScriptPlayerId, script_boundary_pair, start_lua_host,
};
use tokio::sync::mpsc;

use super::SessionRegistry;
use super::outbound::OutboundCommand;
use super::script_menu_endpoint::{
    ScriptMenuCloseRequest, ScriptMenuRouteError, publish_script_menu_click,
};
use crate::login::LoggedInProfile;
use crate::play::PlayerPose;
use crate::play::containers::{ScriptMenuClick, ScriptMenuClickDisposition, ScriptMenuWindow};
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
async fn admitted_open_and_close_preserve_plugin_menu_and_player_identity() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(4);
    let (session_id, _) = registry.register(
        &profile("MenuAdmission"),
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let plugins = tempfile::tempdir().unwrap();
    let plugin = plugins.path().join("catalog");
    std::fs::create_dir(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"id = "catalog"
name = "Catalog"
version = "0.1.0"
api = "0.6.0"
events = ["server.started"]
capabilities = ["inventory_menus"]
"#,
    )
    .unwrap();
    std::fs::write(
        plugin.join("main.lua"),
        format!(
            r#"
function on_server_started(_event)
    solaris.open_inventory_menu({session_id}, "main", "Catalog", {{
        {{slot = 0, resource = "minecraft:apple", count = 1, label = "Apple"}}
    }})
    solaris.close_inventory_menu({session_id}, "main")
end
"#
        ),
    )
    .unwrap();
    let (boundary, host) = start_lua_host(LuaHostConfig::new(plugins.path())).unwrap();
    assert_eq!(host.loaded_plugins(), 1);
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();

    for _ in 0..2 {
        let command = boundary.recv_command().await.unwrap();
        let admitted = boundary.accept_host_command(command).unwrap();
        registry.route_script_menu_command(admitted).unwrap();
    }

    match rx.recv().await.unwrap() {
        OutboundCommand::OpenScriptMenu(request) => {
            assert_eq!(request.owner.plugin_id(), "catalog");
            assert_eq!(request.player_id, ScriptPlayerId::new(session_id));
            assert_eq!(request.menu.id(), "main");
            assert_eq!(request.menu.title(), "Catalog");
            assert_eq!(request.menu.slots()[0].item().label(), Some("Apple"));
            let window = ScriptMenuWindow::open(
                4,
                request.owner,
                request.player_id,
                request.menu,
                &solaris_required_items(),
            )
            .unwrap();
            assert_eq!(window.menu_type(), 0);
            assert_eq!(window.rows(), 1);
            assert_eq!(
                window
                    .wire_items(&crate::play::inventory::PlayerInventory::empty())
                    .len(),
                45
            );
            assert_eq!(
                window
                    .click(
                        ScriptMenuClick::from_packet(4, 1, 4, 1, 0, ContainerInput::Pickup, 0,),
                        ScriptPlayerId::new(session_id + 1),
                        ScriptPlayerContext::new(
                            "wrong-player",
                            "WrongPlayer",
                            false,
                            0.5,
                            64.0,
                            0.5,
                        ),
                    )
                    .unwrap_err(),
                ScriptMenuClickDisposition::Resync
            );
            let event = window
                .click(
                    ScriptMenuClick::from_packet(4, 1, 4, 1, 0, ContainerInput::Pickup, 0),
                    ScriptPlayerId::new(session_id),
                    ScriptPlayerContext::new("player-uuid", "MenuAdmission", false, 0.5, 64.0, 0.5),
                )
                .unwrap();
            assert_eq!(event.target_plugin_id(), Some("catalog"));
            assert!(matches!(
                event.kind(),
                ScriptEventKind::InventoryMenuClicked {
                    player_id,
                    menu_id,
                    slot: 0,
                    click: ScriptInventoryClick::Primary,
                    ..
                } if *player_id == ScriptPlayerId::new(session_id) && menu_id == "main"
            ));
        }
        other => panic!("expected script menu open, got {other:?}"),
    }
    match rx.recv().await.unwrap() {
        OutboundCommand::CloseScriptMenu(request) => {
            assert_eq!(request.plugin_id, "catalog");
            assert_eq!(request.player_id, ScriptPlayerId::new(session_id));
            assert_eq!(request.menu_id, "main");
        }
        other => panic!("expected script menu close, got {other:?}"),
    }

    drop(boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .unwrap()
        .unwrap();
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
