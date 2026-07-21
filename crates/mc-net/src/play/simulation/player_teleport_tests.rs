use std::collections::HashSet;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use mc_script::{ScriptPlayerId, ScriptPlayerTeleportRequest, ScriptPosition};
use tokio::sync::mpsc;

use super::super::inventory::PlayerInventory;
use super::super::persistence::PlayerPersistedState;
use super::super::session::OutboundCommand;
use super::*;

#[tokio::test(flavor = "current_thread")]
async fn owner_commit_result_survives_session_waiter_cancellation() {
    let registry = SessionRegistry::new();
    let profile = crate::login::LoggedInProfile {
        uuid: crate::login::offline_uuid("CancelledTeleportWaiter"),
        name: "CancelledTeleportWaiter".to_owned(),
    };
    let (outbound, mut commands) = mpsc::channel(1);
    let initial_pose = PlayerPose::new(0.5, 64.0, 0.5);
    let (session_id, _) =
        registry.register(&profile, (0, 0), 2, HashSet::new(), outbound, initial_pose);
    let persisted = Arc::new(Mutex::new(PlayerPersistedState::new_default(initial_pose)));
    persisted.lock().unwrap().inventory = PlayerInventory::empty();
    registry.register_player_persistence(session_id, Arc::clone(&persisted));

    let target = ScriptPosition::try_new(40.0, 70.0, 1.0).unwrap();
    let request = ScriptPlayerTeleportRequest::try_new(
        "cancelled-waiter",
        ScriptPlayerId::new(session_id),
        target,
    )
    .unwrap();
    let mut routed = Box::pin(registry.route_script_player_teleport(request));
    assert_pending(routed.as_mut()).await;
    let OutboundCommand::ScriptPlayerTeleport(command) = commands.recv().await.unwrap() else {
        panic!("expected script teleport command");
    };
    let (position, completion) = command.into_owner_completion();

    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let session_handle = handle.for_session(session_id);
    let pose = PlayerPose::new(position.x(), position.y(), position.z());
    let mut commit = Box::pin(session_handle.commit_script_player_teleport(pose, completion));
    assert_pending(commit.as_mut()).await;
    assert_eq!(handle.snapshot().depth, 1);

    assert_eq!(owner.process_tick(&registry, 1).processed, 1);
    drop(commit);

    assert_eq!(routed.await, Ok(()));
    let committed = persisted.lock().unwrap().pose;
    assert_eq!((committed.x, committed.y, committed.z), (40.0, 70.0, 1.0));
}

async fn assert_pending<F: Future>(mut future: std::pin::Pin<&mut F>) {
    std::future::poll_fn(|context| {
        assert!(future.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
}
