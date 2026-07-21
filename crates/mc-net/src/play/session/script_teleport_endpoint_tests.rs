use std::collections::HashSet;
use std::future::Future;
use std::task::Poll;

use mc_script::{
    ScriptPlayerId, ScriptPlayerTeleportFailure, ScriptPlayerTeleportRequest, ScriptPosition,
};
use tokio::sync::mpsc;

use super::SessionRegistry;
use super::outbound::OutboundCommand;
use crate::login::LoggedInProfile;
use crate::play::PlayerPose;
use crate::play::simulation::{
    CommittedPlayerPose, SimulationCommand, SimulationRequestError, SimulationResponse,
};

fn profile(name: &str) -> LoggedInProfile {
    LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_owned(),
    }
}

fn request(player_id: u64) -> ScriptPlayerTeleportRequest {
    ScriptPlayerTeleportRequest::try_new(
        "warp-home",
        ScriptPlayerId::new(player_id),
        ScriptPosition::try_new(12.5, 70.0, -4.5).unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn connected_player_teleport_waits_for_exact_session_owner_completion() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(1);
    let (session_id, _) = registry.register(
        &profile("TeleportOwner"),
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let mut routed = Box::pin(registry.route_script_player_teleport(request(session_id)));

    std::future::poll_fn(|context| {
        assert!(routed.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    let OutboundCommand::ScriptPlayerTeleport(command) = rx.recv().await.unwrap() else {
        panic!("expected script player teleport command");
    };
    assert_eq!(
        command.position,
        ScriptPosition::try_new(12.5, 70.0, -4.5).unwrap()
    );
    command.complete(Ok(()));
    assert_eq!(routed.await, Ok(()));
}

#[tokio::test]
async fn simulation_owner_completion_survives_session_publication_cancellation() {
    assert_eq!(
        route_with_owner_outcome(Ok(SimulationResponse::PlayerPose(Ok(
            CommittedPlayerPose {
                food: 20,
                saturation: 5.0,
                exhaustion: 0.0,
                resources_changed: false,
            },
        ))))
        .await,
        Ok(())
    );
}

#[tokio::test]
async fn stale_session_owner_outcome_reports_player_unavailable() {
    assert_eq!(
        route_with_owner_outcome(Ok(SimulationResponse::PlayerPose(Err(
            SimulationRequestError::StaleSession,
        ))))
        .await,
        Err(ScriptPlayerTeleportFailure::PlayerUnavailable)
    );
}

#[tokio::test]
async fn unavailable_simulation_owner_reports_runtime_unavailable() {
    assert_eq!(
        route_with_owner_outcome(Err(SimulationRequestError::Closed)).await,
        Err(ScriptPlayerTeleportFailure::RuntimeUnavailable)
    );
}

async fn route_with_owner_outcome(
    outcome: Result<SimulationResponse, SimulationRequestError>,
) -> Result<(), ScriptPlayerTeleportFailure> {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(1);
    let (session_id, _) = registry.register(
        &profile("CancelledTeleportPublication"),
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let mut routed = Box::pin(registry.route_script_player_teleport(request(session_id)));
    std::future::poll_fn(|context| {
        assert!(routed.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    let OutboundCommand::ScriptPlayerTeleport(command) = rx.recv().await.unwrap() else {
        panic!("expected script player teleport command");
    };

    let (position, completion) = command.into_owner_completion();
    let mut owner_command = SimulationCommand::CommitPlayerPose {
        actor_session: session_id,
        pose: PlayerPose::new(position.x(), position.y(), position.z()),
        exhaustion: 0.0,
        script_teleport_completion: Some(completion),
    };
    owner_command.complete_script_player_teleport(&outcome);

    routed.await
}

#[tokio::test]
async fn missing_or_closed_player_rejects_without_waiting() {
    let registry = SessionRegistry::new();
    assert_eq!(
        registry.route_script_player_teleport(request(77)).await,
        Err(ScriptPlayerTeleportFailure::PlayerUnavailable)
    );

    let (tx, rx) = mpsc::channel(1);
    let (session_id, _) = registry.register(
        &profile("ClosedTeleportLane"),
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    drop(rx);
    assert_eq!(
        registry
            .route_script_player_teleport(request(session_id))
            .await,
        Err(ScriptPlayerTeleportFailure::PlayerUnavailable)
    );
}

#[tokio::test]
async fn player_teleport_keeps_reliable_order_under_session_pressure() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(1);
    tx.try_send(OutboundCommand::SystemChat {
        message: "first".to_owned(),
    })
    .unwrap();
    let (session_id, _) = registry.register(
        &profile("TeleportPressure"),
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let mut routed = Box::pin(registry.route_script_player_teleport(request(session_id)));
    std::future::poll_fn(|context| {
        assert!(routed.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;

    assert!(matches!(
        rx.recv().await.unwrap(),
        OutboundCommand::SystemChat { message } if message == "first"
    ));
    let OutboundCommand::ScriptPlayerTeleport(command) = rx.recv().await.unwrap() else {
        panic!("expected pressured script player teleport command");
    };
    command.complete(Err(ScriptPlayerTeleportFailure::TeleportPending));
    assert_eq!(
        routed.await,
        Err(ScriptPlayerTeleportFailure::TeleportPending)
    );
}
