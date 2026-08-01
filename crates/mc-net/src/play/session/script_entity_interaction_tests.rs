use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use mc_entity::{EntityId, Vec3};
use mc_protocol::packets::play::GameMode;
use tokio::sync::mpsc;

use super::*;
use crate::login::LoggedInProfile;
use crate::play::PlayerPose;
use crate::play::persistence::PlayerPersistedState;

fn register_player(
    registry: &SessionRegistry,
    name: &str,
) -> (SessionId, Arc<Mutex<PlayerPersistedState>>) {
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_owned(),
    };
    let (outbound, _receiver) = mpsc::channel(8);
    let session = registry
        .register(&profile, (0, 0), 2, HashSet::new(), outbound, pose)
        .0;
    let persisted = Arc::new(Mutex::new(PlayerPersistedState::new_default(pose)));
    registry.register_player_persistence(session, Arc::clone(&persisted));
    (session, persisted)
}

fn spawn_entity(registry: &SessionRegistry, entity_type: &str, position: Vec3) -> EntityId {
    let before = registry
        .persisted_entity_records()
        .into_iter()
        .map(|record| record.snapshot.id)
        .collect::<HashSet<_>>();
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        5,
        entity_type.to_owned(),
        position,
    );
    registry
        .persisted_entity_records()
        .into_iter()
        .map(|record| record.snapshot.id)
        .find(|id| !before.contains(id))
        .expect("spawned entity remains authoritative")
}

#[test]
fn owner_accepts_only_reachable_live_entity_interactions() {
    let registry = SessionRegistry::new();
    let (player, persisted) = register_player(&registry, "EntityInteractor");
    let reachable = spawn_entity(&registry, "minecraft:villager", Vec3::new(1.5, 64.0, 0.5));
    let far = spawn_entity(&registry, "minecraft:villager", Vec3::new(20.5, 64.0, 0.5));
    let nonliving = spawn_entity(&registry, "minecraft:item", Vec3::new(1.5, 64.0, 0.5));
    let dying = spawn_entity(&registry, "minecraft:villager", Vec3::new(2.5, 64.0, 0.5));
    let reachable_snapshot = registry
        .server_entity_snapshot(reachable)
        .expect("reachable entity snapshot");
    assert_eq!(reachable_snapshot.health, Some(20.0));
    assert!(within_entity_reach(
        PlayerPose::new(0.5, 64.0, 0.5),
        reachable_snapshot.position,
        entity_aabb(&reachable_snapshot.type_name),
        GameMode::Survival,
    ));
    assert_eq!(
        persisted.lock().expect("test lock poisoned").game_mode,
        GameMode::Survival
    );

    let accepted = registry
        .accept_script_entity_interaction(player, reachable)
        .expect("reachable living entity interaction");
    assert_eq!(
        (
            accepted.player_pose.x,
            accepted.player_pose.y,
            accepted.player_pose.z,
        ),
        (0.5, 64.0, 0.5)
    );
    assert_eq!(accepted.game_mode, GameMode::Survival);
    assert_eq!(accepted.entity_id, reachable);
    assert_eq!(accepted.entity_type, "minecraft:villager");
    assert_eq!(accepted.entity_position, Vec3::new(1.5, 64.0, 0.5));

    assert!(
        registry
            .accept_script_entity_interaction(player, far)
            .is_none()
    );
    assert!(
        registry
            .accept_script_entity_interaction(player, nonliving)
            .is_none()
    );
    assert!(
        registry
            .damage_server_entity_for_test(dying, 100.0)
            .is_some()
    );
    assert!(
        registry
            .accept_script_entity_interaction(player, dying)
            .is_none()
    );
    assert!(
        registry
            .accept_script_entity_interaction(player, EntityId(i32::MAX))
            .is_none()
    );

    persisted.lock().expect("test lock poisoned").game_mode = GameMode::Spectator;
    assert!(
        registry
            .accept_script_entity_interaction(player, reachable)
            .is_none()
    );

    for game_mode in [GameMode::Creative, GameMode::Adventure] {
        persisted.lock().expect("test lock poisoned").game_mode = game_mode;
        assert_eq!(
            registry
                .accept_script_entity_interaction(player, reachable)
                .expect("creative and adventure interactions remain accepted")
                .game_mode,
            game_mode
        );
    }

    let mut state = persisted.lock().expect("test lock poisoned");
    state.game_mode = GameMode::Survival;
    state.survival.health = 0.0;
    drop(state);
    assert!(
        registry
            .accept_script_entity_interaction(player, reachable)
            .is_none()
    );
}
