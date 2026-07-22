use super::*;

fn register_at(registry: &SessionRegistry, name: &str, pose: PlayerPose) -> SessionId {
    let (tx, _rx) = mpsc::channel(8);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_string(),
    };
    registry
        .register(&profile, (0, 0), 12, HashSet::new(), tx, pose)
        .0
}

#[test]
fn spawning_chunks_use_vanilla_center_distance_and_ignore_spectators() {
    let registry = SessionRegistry::new();
    let player = register_at(&registry, "Inhabited", PlayerPose::new(0.5, 64.0, 0.5));
    let near = (7, 0);
    let far = (8, 0);
    registry.mark_loaded(player, near);
    registry.mark_loaded(player, far);

    assert_eq!(registry.spawning_chunks_sorted(), vec![near]);

    registry
        .lock_inner("make inhabited-time test player spectator")
        .spectator_sessions
        .insert(player);
    assert!(registry.spawning_chunks_sorted().is_empty());
}
