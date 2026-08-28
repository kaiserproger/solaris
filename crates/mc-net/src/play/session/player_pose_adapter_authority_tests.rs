use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use mc_data::blocks::solaris_required_blocks_report;
use tokio::sync::mpsc;

use super::*;
use crate::login::LoggedInProfile;
use crate::play::persistence::PlayerPersistedState;
use crate::play::tests::{fluid_test_facts, insert_fluid_test_chunk, interaction_state_for_blocks};

fn pose_tuple(pose: PlayerPose) -> (f64, f64, f64, f32, f32) {
    (pose.x, pose.y, pose.z, pose.yaw, pose.pitch)
}

#[tokio::test]
async fn authority_rejects_large_movement_without_mutation_but_allows_explicit_teleport() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&solaris_required_blocks_report())
            .expect("embedded vanilla registry builds"),
    );
    let mut world = interaction_state_for_blocks(Arc::clone(&blocks));
    world.block_facts = Arc::new(fluid_test_facts());
    insert_fluid_test_chunk(&world).await;
    let movement_authority = PlayerMovementAuthorityResources::new(
        world.world_read.clone(),
        blocks,
        Arc::clone(&world.block_facts),
    );

    let registry = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("MovementAuthorityAlice"),
        name: "MovementAuthorityAlice".to_owned(),
    };
    let old_pose = PlayerPose::new(0.5, 64.0, 0.5);
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) =
        registry.register(&profile, (0, 0), 2, HashSet::from([(0, 0)]), tx, old_pose);
    registry.mark_loaded(session_id, (0, 0));
    let persisted = Arc::new(Mutex::new(PlayerPersistedState::new_default(old_pose)));
    registry.register_player_persistence(session_id, Arc::clone(&persisted));
    let destination = PlayerPose::new(20.5, 64.0, 0.5);

    let rejected = registry.commit_player_pose_request(
        &SimulationAuthority::for_test(),
        PlayerPoseCommitRequest {
            actor_session: session_id,
            kind: PlayerPoseCommitKind::Movement,
            pose: destination,
            exhaustion: 1.0,
        },
        Some(&movement_authority),
    );
    assert_eq!(
        rejected.unwrap_err(),
        SimulationRequestError::PlayerMovementRejected(PlayerMovementRejection::Displacement)
    );
    let session_pose = registry
        .inner
        .lock()
        .expect("session registry lock")
        .sessions
        .get(&session_id)
        .expect("active session")
        .pose;
    assert_eq!(pose_tuple(session_pose), pose_tuple(old_pose));
    let state = persisted.lock().expect("persisted player lock");
    assert_eq!(pose_tuple(state.pose), pose_tuple(old_pose));
    assert_eq!(state.survival.exhaustion, 0.0);
    drop(state);

    registry
        .commit_player_pose_request(
            &SimulationAuthority::for_test(),
            PlayerPoseCommitRequest {
                actor_session: session_id,
                kind: PlayerPoseCommitKind::Teleport,
                pose: destination,
                exhaustion: 0.0,
            },
            Some(&movement_authority),
        )
        .expect("explicit teleport bypasses ordinary movement fences");
    let session_pose = registry
        .inner
        .lock()
        .expect("session registry lock")
        .sessions
        .get(&session_id)
        .expect("active session")
        .pose;
    assert_eq!(pose_tuple(session_pose), pose_tuple(destination));
    assert_eq!(
        pose_tuple(persisted.lock().expect("persisted player lock").pose),
        pose_tuple(destination)
    );
}

#[tokio::test]
async fn respawn_pose_uses_teleport_authority_when_spawn_chunk_is_not_in_old_view() {
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&solaris_required_blocks_report())
            .expect("embedded vanilla registry builds"),
    );
    let world = interaction_state_for_blocks(Arc::clone(&blocks));
    let movement_authority = PlayerMovementAuthorityResources::new(
        world.world_read.clone(),
        blocks,
        Arc::clone(&world.block_facts),
    );

    let registry = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("FarDeathRespawn"),
        name: "FarDeathRespawn".to_owned(),
    };
    let death_pose = PlayerPose::new(16.5, 64.0, 0.5);
    let respawn_pose = PlayerPose::new(8.5, 64.0, 0.5);
    let (tx, _rx) = mpsc::channel(8);
    let (session_id, _) =
        registry.register(&profile, (1, 0), 2, HashSet::from([(1, 0)]), tx, death_pose);
    registry.mark_loaded(session_id, (1, 0));
    let persisted = Arc::new(Mutex::new(PlayerPersistedState::new_default(death_pose)));
    registry.register_player_persistence(session_id, Arc::clone(&persisted));

    let movement = registry.commit_player_pose_request(
        &SimulationAuthority::for_test(),
        PlayerPoseCommitRequest {
            actor_session: session_id,
            kind: PlayerPoseCommitKind::Movement,
            pose: respawn_pose,
            exhaustion: 0.0,
        },
        Some(&movement_authority),
    );
    assert_eq!(
        movement.unwrap_err(),
        SimulationRequestError::PlayerMovementRejected(
            PlayerMovementRejection::DestinationUnloaded
        )
    );

    registry
        .commit_player_pose_request(
            &SimulationAuthority::for_test(),
            PlayerPoseCommitRequest {
                actor_session: session_id,
                kind: PlayerPoseCommitKind::Teleport,
                pose: respawn_pose,
                exhaustion: 0.0,
            },
            Some(&movement_authority),
        )
        .expect("server-authoritative respawn must not depend on the pre-death loaded window");
    assert_eq!(
        pose_tuple(persisted.lock().expect("persisted player lock").pose),
        pose_tuple(respawn_pose)
    );
}
