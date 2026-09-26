use std::collections::{BTreeSet, HashSet};

use std::sync::Arc;
use tokio::sync::mpsc;

use super::outbound::OutboundCommand;
use super::{SessionRegistration, SessionRegistry};
use crate::login::LoggedInProfile;
use crate::play::{GameMode, PlayerPose};

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

#[test]
fn plugin_player_context_tracks_live_operator_revocation() {
    let config = crate::server::CommandPermissionConfig::new(["Builder"], false);
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel::<OutboundCommand>(1);
    let builder = profile("Builder");
    registry
        .try_register(SessionRegistration {
            profile: &builder,
            properties: &[],
            center: (0, 0),
            view_distance: 2,
            desired: HashSet::new(),
            tx,
            pose: PlayerPose::new(1.0, 64.0, 2.0),
            game_mode: GameMode::Survival,
            max_sessions: 8,
            script_permissions: config.clone(),
            peer: "192.168.1.20:40000".parse().unwrap(),
            dimension: "minecraft:overworld",
            loader_session: None,
        })
        .unwrap();

    let (players, _) = registry.script_online_players(1).unwrap();
    assert!(players[0].context().operator());
    config
        .operator_identities()
        .store(Arc::new(BTreeSet::new()));
    let (players, _) = registry.script_online_players(1).unwrap();
    assert!(!players[0].context().operator());
}

#[test]
fn player_sessions_lookup_resolves_the_advertised_identity() {
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

    // Ask with what the registry advertises and require the session it reports:
    // the lookup and the online-player snapshot are one authority, in both
    // directions.
    let (players, _) = registry.script_online_players(8).unwrap();
    assert_eq!(players.len(), 2);
    for player in &players {
        assert_eq!(
            registry.script_session_of_identity(player.context().uuid()),
            Some(player.player_id().value()),
            "identity {} must resolve to the session the snapshot reports",
            player.context().uuid()
        );
    }
    assert_ne!(alice_id, bob_id);

    // A username is not an identity, and an unknown identity belongs to nobody.
    assert_eq!(registry.script_session_of_identity("Alice"), None);
    assert_eq!(
        registry.script_session_of_identity(&crate::login::offline_uuid("Carol").to_string()),
        None
    );
    assert_eq!(registry.script_session_of_identity(""), None);
    assert_eq!(registry.script_session_of_identity("not-a-uuid"), None);
}

#[test]
fn player_sessions_lookup_answers_absent_after_the_connection_ended() {
    let registry = SessionRegistry::new();
    let alice = crate::login::offline_uuid("Alice").to_string();
    let (alice_tx, alice_rx) = mpsc::channel::<OutboundCommand>(1);
    let (alice_id, _) = registry.register(
        &profile("Alice"),
        (0, 0),
        2,
        HashSet::new(),
        alice_tx,
        PlayerPose::new(1.0, 64.0, 2.0),
    );
    assert_eq!(registry.script_session_of_identity(&alice), Some(alice_id));

    // The connection is gone but its session is not torn down yet: the player
    // is answered as absent, not with the session they used to hold, and the
    // snapshot agrees.
    drop(alice_rx);
    assert_eq!(registry.script_session_of_identity(&alice), None);
    assert!(registry.script_online_players(8).unwrap().0.is_empty());

    let bob = crate::login::offline_uuid("Bob").to_string();
    let (bob_tx, _bob_rx) = mpsc::channel::<OutboundCommand>(1);
    let (bob_id, _) = registry.register(
        &profile("Bob"),
        (0, 0),
        2,
        HashSet::new(),
        bob_tx,
        PlayerPose::new(3.0, 65.0, 4.0),
    );
    assert_eq!(registry.script_session_of_identity(&bob), Some(bob_id));

    // Once the session is torn down the identity stays absent, and the next
    // player to hold it is the one that is answered.
    registry.unregister(bob_id);
    assert_eq!(registry.script_session_of_identity(&bob), None);
    let (returning_tx, _returning_rx) = mpsc::channel::<OutboundCommand>(1);
    let (returning_id, _) = registry.register(
        &profile("Bob"),
        (0, 0),
        2,
        HashSet::new(),
        returning_tx,
        PlayerPose::new(5.0, 66.0, 6.0),
    );
    assert_ne!(returning_id, bob_id);
    assert_eq!(
        registry.script_session_of_identity(&bob),
        Some(returning_id)
    );
}
