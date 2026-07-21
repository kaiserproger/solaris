use std::collections::HashSet;

use tokio::sync::mpsc;

use super::SessionRegistry;
use super::outbound::OutboundCommand;
use crate::login::LoggedInProfile;
use crate::play::PlayerPose;

fn profile(name: &str) -> LoggedInProfile {
    LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_owned(),
    }
}

#[test]
fn online_player_snapshot_is_sorted_bounded_and_excludes_closed_sessions() {
    let registry = SessionRegistry::new();
    let (alice_tx, _alice_rx) = mpsc::channel::<OutboundCommand>(1);
    let (alice_id, _) = registry.register(
        &profile("Alice"),
        (0, 0),
        2,
        HashSet::new(),
        alice_tx,
        PlayerPose::new(1.0, 64.0, 2.0),
    );
    let (bob_tx, _bob_rx) = mpsc::channel::<OutboundCommand>(1);
    let (bob_id, _) = registry.register(
        &profile("Bob"),
        (0, 0),
        2,
        HashSet::new(),
        bob_tx,
        PlayerPose::new(3.0, 65.0, 4.0),
    );
    let (closed_tx, closed_rx) = mpsc::channel::<OutboundCommand>(1);
    registry.register(
        &profile("Closed"),
        (0, 0),
        2,
        HashSet::new(),
        closed_tx,
        PlayerPose::new(5.0, 66.0, 6.0),
    );
    drop(closed_rx);

    let (players, truncated) = registry.script_online_players(1).unwrap();
    assert_eq!(players.len(), 1);
    assert_eq!(players[0].player_id().value(), alice_id.min(bob_id));
    assert_eq!(players[0].context().username(), "Alice");
    assert_eq!(players[0].dimension(), "minecraft:overworld");
    assert!(truncated);
}
