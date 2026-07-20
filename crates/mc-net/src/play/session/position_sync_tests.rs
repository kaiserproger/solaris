use std::collections::HashSet;

use mc_entity::{EntityItemStack, Vec3};
use tokio::sync::mpsc;

use crate::login::{LoggedInProfile, offline_uuid};

use super::visibility::{EntityPositionUpdate, entity_wire_move_for_kind};
use super::{OutboundCommand, PlayerPose, SessionRegistry, dispatch_visibility_commands};
use crate::play::simulation::SimulationAuthority;
use crate::play::wire_entities::ServerEntityWireMove;
use crate::play::{ENTITY_MOVE_SEND_INTERVAL_TICKS, EntityPhysicsStep};

#[test]
fn large_entity_delta_dispatches_absolute_position_sync() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(16);
    let profile = LoggedInProfile {
        uuid: offline_uuid("AbsoluteEntityObserver"),
        name: "AbsoluteEntityObserver".to_owned(),
    };
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let spawn = registry.spawn_item_drop(1, Vec3::new(0.5, 64.0, 0.5), EntityItemStack::new(42, 1));
    let entity_id = spawn
        .iter()
        .find_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("visible item spawn");
    dispatch_visibility_commands(spawn);
    while rx.try_recv().is_ok() {}

    registry.apply_entity_physics_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &[EntityPhysicsStep {
            id: entity_id,
            position: Vec3::new(9.0, 64.0, 0.5),
            velocity: Vec3::ZERO,
            on_ground: false,
            horizontal_collision: false,
        }],
    );

    let movement = std::iter::from_fn(|| rx.try_recv().ok())
        .find_map(|command| match command {
            OutboundCommand::MoveEntityRelative(movement) => Some(movement),
            _ => None,
        })
        .expect("absolute movement dispatch");
    assert_eq!(
        movement.wire_move,
        Some(ServerEntityWireMove::Absolute {
            position: Vec3::new(9.0, 64.0, 0.5),
        })
    );
}

#[test]
fn arrows_use_position_rotation_packets_for_position_or_body_rotation_updates() {
    let delta = Vec3::new(1.0 / 4096.0, 0.0, 0.0);
    assert_eq!(
        entity_wire_move_for_kind(
            EntityPositionUpdate::Relative(delta),
            false,
            Vec3::ZERO,
            true,
        ),
        Some(ServerEntityWireMove::PositionRotation { delta })
    );
    assert_eq!(
        entity_wire_move_for_kind(EntityPositionUpdate::None, true, Vec3::ZERO, true),
        Some(ServerEntityWireMove::PositionRotation { delta: Vec3::ZERO })
    );
}

#[test]
fn scheduled_updates_advance_global_counters_without_movement_reset() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(256);
    let profile = LoggedInProfile {
        uuid: offline_uuid("GlobalRefreshObserver"),
        name: "GlobalRefreshObserver".to_owned(),
    };
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        tx,
        PlayerPose::new(4.5, 64.0, 4.5),
    );
    assert!(registry.mark_loaded(session_id, (0, 0)).is_empty());
    let spawn = registry.spawn_item_drop(1, Vec3::new(0.5, 64.0, 0.5), EntityItemStack::new(42, 1));
    let entity_id = spawn
        .iter()
        .find_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("visible item spawn");
    dispatch_visibility_commands(spawn);
    while rx.try_recv().is_ok() {}

    let initial = registry
        .server_entity_snapshot(entity_id)
        .expect("spawned item snapshot");
    let unchanged_step = EntityPhysicsStep {
        id: entity_id,
        position: initial.position,
        velocity: initial.velocity,
        on_ground: initial.on_ground,
        horizontal_collision: false,
    };

    {
        let inner = registry.lock_inner("verify initial tracker counters");
        let state = inner
            .last_sent_entity_states
            .get(&entity_id)
            .expect("item wire state");
        assert_eq!(state.tracking_update_count, 0);
        assert_eq!(state.teleport_delay, 0);
    }

    let mut position = initial.position;
    let mut movement_updates = Vec::new();
    for update_index in 0_u64..=60 {
        if update_index == 30 {
            position.x += 0.25;
        }
        registry.apply_entity_physics_and_dispatch(
            (update_index + 1) * ENTITY_MOVE_SEND_INTERVAL_TICKS,
            &[EntityPhysicsStep {
                position,
                ..unchanged_step
            }],
        );
        while let Ok(command) = rx.try_recv() {
            if let OutboundCommand::MoveEntityRelative(movement) = command
                && movement.id == entity_id
                && movement.wire_move.is_some()
            {
                movement_updates.push((update_index, movement.wire_move));
            }
        }
    }

    assert_eq!(
        movement_updates,
        vec![
            (
                0,
                Some(ServerEntityWireMove::Position { delta: Vec3::ZERO }),
            ),
            (
                30,
                Some(ServerEntityWireMove::Position {
                    delta: Vec3::new(0.25, 0.0, 0.0),
                }),
            ),
            (
                60,
                Some(ServerEntityWireMove::Position { delta: Vec3::ZERO }),
            ),
        ]
    );
    let inner = registry.lock_inner("verify scheduled tracker counters");
    let state = inner
        .last_sent_entity_states
        .get(&entity_id)
        .expect("item wire state");
    assert_eq!(state.tracking_update_count, 61);
    assert_eq!(state.teleport_delay, 61);
}

#[test]
fn player_body_push_does_not_advance_tracker_counters() {
    let registry = SessionRegistry::new();
    let (player_tx, _player_rx) = mpsc::channel(32);
    let player_profile = LoggedInProfile {
        uuid: offline_uuid("CounterPushPlayer"),
        name: "CounterPushPlayer".to_owned(),
    };
    let (player, _) = registry.register(
        &player_profile,
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        player_tx,
        PlayerPose::new(0.0, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    let (observer_tx, _observer_rx) = mpsc::channel(32);
    let observer_profile = LoggedInProfile {
        uuid: offline_uuid("CounterPushObserver"),
        name: "CounterPushObserver".to_owned(),
    };
    let (observer, _) = registry.register(
        &observer_profile,
        (0, 0),
        2,
        HashSet::from([(0, 0)]),
        observer_tx,
        PlayerPose::new(3.0, 64.0, 0.5),
    );
    let _ = registry.mark_loaded(observer, (0, 0));
    let entity_id = registry
        .spawn_command_entity(
            &SimulationAuthority::for_test(),
            1,
            "minecraft:zombie".to_owned(),
            Vec3::new(0.6, 64.0, 0.5),
        )
        .into_iter()
        .find_map(|dispatch| match dispatch.command {
            OutboundCommand::SpawnEntity(entity) => Some(entity.id),
            _ => None,
        })
        .expect("visible zombie spawn");
    let snapshot = registry
        .server_entity_snapshot(entity_id)
        .expect("spawned zombie snapshot");

    for update_index in 0_u64..3 {
        registry.apply_entity_physics_and_dispatch(
            (update_index + 1) * ENTITY_MOVE_SEND_INTERVAL_TICKS,
            &[EntityPhysicsStep {
                id: entity_id,
                position: snapshot.position,
                velocity: snapshot.velocity,
                on_ground: snapshot.on_ground,
                horizontal_collision: false,
            }],
        );
    }
    let counters_before = {
        let inner = registry.lock_inner("capture tracker counters before body push");
        let state = inner
            .last_sent_entity_states
            .get(&entity_id)
            .expect("zombie wire state");
        (state.tracking_update_count, state.teleport_delay)
    };

    let dispatches = registry.update_pose(player, PlayerPose::new(0.3, 64.0, 0.5));

    assert!(dispatches.iter().any(|dispatch| {
        matches!(
            &dispatch.command,
            OutboundCommand::MoveEntityRelative(movement)
                if movement.id == entity_id
                    && movement.wire_move.is_none()
                    && movement.velocity != Vec3::ZERO
        )
    }));
    let inner = registry.lock_inner("verify body push tracker counters");
    let state = inner
        .last_sent_entity_states
        .get(&entity_id)
        .expect("zombie wire state");
    assert_eq!(
        (state.tracking_update_count, state.teleport_delay),
        counters_before
    );
}
