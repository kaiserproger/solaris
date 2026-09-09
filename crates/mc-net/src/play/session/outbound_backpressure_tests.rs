use std::collections::HashSet;

use tokio::sync::mpsc;

use crate::play::wire_entities::ServerEntityWireMove;

use super::*;

fn test_pressure(registry: &SessionRegistry) -> Arc<OutboundPressureMetrics> {
    Arc::clone(&registry.outbound_pressure)
}

fn test_recipient(
    registry: &SessionRegistry,
    id: SessionId,
    tx: mpsc::Sender<OutboundCommand>,
) -> SessionRecipient {
    SessionRecipient::unordered(id, tx, test_pressure(registry))
}

#[test]
fn loaded_chunk_batches_entity_spawns_before_bounded_outbound_queue() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("ChunkSpawnBatch"),
        name: "ChunkSpawnBatch".to_owned(),
    };
    let (session, _) = registry.register(
        &profile,
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );

    for offset in 0..17 {
        assert!(
            registry
                .spawn_command_entity(
                    &SimulationAuthority::for_test(),
                    4,
                    "minecraft:cow".to_owned(),
                    Vec3::new(0.5 + f64::from(offset) * 0.01, 64.0, 0.5),
                )
                .is_empty()
        );
    }

    let dispatches = registry.mark_loaded(session, (0, 0));
    assert!(matches!(
        dispatches.as_slice(),
        [VisibilityDispatch {
            command: OutboundCommand::SpawnEntities(entities),
            ..
        }] if entities.len() == 17
    ));

    dispatch_visibility_commands(dispatches);
    assert!(matches!(
        rx.try_recv(),
        Ok(OutboundCommand::SpawnEntities(entities)) if entities.len() == 17
    ));
    assert!(rx.try_recv().is_err());
    let pressure = registry.pressure_snapshot();
    assert_eq!(pressure.entity_dispatches.spawn, 17);
    assert_eq!(pressure.reliable_command_drops, 0);
    assert_eq!(pressure.slow_client_pressure_sheds, 0);
}

#[tokio::test]
async fn unloading_dense_chunk_does_not_disconnect_reliable_session() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(1);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("ChunkUnloadBurst"),
        name: "ChunkUnloadBurst".to_owned(),
    };
    let (session, _) = registry.register(
        &profile,
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    for index in 0..400 {
        registry.spawn_command_entity(
            &SimulationAuthority::for_test(),
            4,
            "minecraft:cow".to_owned(),
            Vec3::new(f64::from(index % 2) * 16.0 + 0.5, 64.0, 0.5),
        );
    }
    dispatch_visibility_commands(registry.mark_loaded(session, (0, 0)));
    dispatch_visibility_commands(registry.mark_loaded(session, (1, 0)));
    // The receiver has not consumed the initial population when its view moves.
    dispatch_visibility_commands(registry.mark_unloaded(session, &[(0, 0), (1, 0)]));
    let pressure = registry.pressure_snapshot();
    assert_eq!(pressure.slow_client_pressure_sheds, 0);
    assert_eq!(pressure.reliable_command_drops, 0);
    let mut visible = HashSet::new();
    for _ in 0..2 {
        let Some(OutboundCommand::SpawnEntities(entities)) = rx.recv().await else {
            panic!("initial visible population was lost");
        };
        visible.extend(entities.iter().map(|entity| entity.id.0));
    }
    while !visible.is_empty() {
        let removed = match rx.recv().await.expect("removal delivery") {
            OutboundCommand::DespawnEntity(entity) => vec![entity.id.0],
            OutboundCommand::DespawnEntities(entity_ids) => entity_ids,
            other => panic!("unexpected command before removals complete: {other:?}"),
        };
        for entity_id in removed {
            assert!(visible.remove(&entity_id), "unknown or duplicated removal");
        }
    }
    assert_eq!(
        registry.persisted_entity_records().len(),
        400,
        "leaving a view must not delete authoritative entities"
    );
}

#[tokio::test]
async fn dense_entity_movement_backlog_coalesces_without_disconnect() {
    const ENTITY_COUNT: usize = 5_132;

    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(16);
    for entity_id in 1..=16 {
        tx.try_send(OutboundCommand::AnimatePlayer { entity_id })
            .expect("fill recipient queue");
    }
    let recipient = test_recipient(&registry, 71, tx);
    let pressure = test_pressure(&registry);
    let retry_completed = pressure.reliable_retry_completed.notified();
    let first_dequeued = pressure.reliable_retry_dequeued.notified();
    tokio::pin!(first_dequeued);
    first_dequeued.as_mut().enable();

    let movement_batch = |tick: usize| {
        (0..ENTITY_COUNT)
            .map(|index| ServerEntityMove {
                id: EntityId(42 + index as i32),
                position: Vec3::new(tick as f64, 64.0, index as f64),
                wire_move: Some(ServerEntityWireMove::Position {
                    delta: Vec3::new(0.25, 0.0, 0.0),
                }),
                velocity: Vec3::new(tick as f64, 0.0, 0.0),
                rotation: Rotation {
                    yaw: tick as f32,
                    pitch: 0.0,
                    head_yaw: tick as f32,
                },
                on_ground: true,
                send_velocity: tick <= 2,
                send_head_rotation: tick == 2,
            })
            .collect::<Vec<_>>()
    };

    dispatch_visibility_command(
        &recipient,
        OutboundCommand::MoveEntitiesRelative(movement_batch(1)),
    );
    first_dequeued.await;

    for tick in 2..=64 {
        dispatch_visibility_command(
            &recipient,
            OutboundCommand::MoveEntitiesRelative(movement_batch(tick)),
        );
    }

    for expected in 1..=16 {
        assert!(matches!(
            rx.recv().await,
            Some(OutboundCommand::AnimatePlayer { entity_id }) if entity_id == expected
        ));
    }
    assert!(matches!(
        rx.recv().await,
        Some(OutboundCommand::MoveEntitiesRelative(movements)) if movements.len() == ENTITY_COUNT
    ));
    let Some(OutboundCommand::MoveEntitiesRelative(movements)) = rx.recv().await else {
        panic!("coalesced movement batch must follow the blocked command");
    };
    assert_eq!(movements.len(), ENTITY_COUNT);
    for (index, movement) in movements.iter().enumerate() {
        assert!(matches!(
            movement,
            ServerEntityMove {
                wire_move: Some(ServerEntityWireMove::Absolute { position }),
                send_velocity: true,
                send_head_rotation: true,
                ..
            } if *position == Vec3::new(64.0, 64.0, index as f64)
        ));
        assert_eq!(movement.velocity, Vec3::new(64.0, 0.0, 0.0));
        assert_eq!(movement.rotation.yaw, 64.0);
        assert_eq!(movement.rotation.head_yaw, 64.0);
    }
    assert!(rx.try_recv().is_err(), "stale movement batches were queued");
    retry_completed.await;
    let snapshot = registry.pressure_snapshot();
    assert_eq!(snapshot.reliable_command_drops, 0);
    assert_eq!(snapshot.slow_client_pressure_sheds, 0);
    assert_eq!(snapshot.reliable_command_retries_in_flight, 0);
}

#[tokio::test]
async fn interleaved_player_and_entity_movements_coalesce_without_disconnect() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(16);
    for entity_id in 1..=16 {
        tx.try_send(OutboundCommand::AnimatePlayer { entity_id })
            .expect("fill recipient queue");
    }
    let recipient = test_recipient(&registry, 72, tx);
    let pressure = test_pressure(&registry);
    let retry_completed = pressure.reliable_retry_completed.notified();
    let first_dequeued = pressure.reliable_retry_dequeued.notified();
    tokio::pin!(first_dequeued);
    first_dequeued.as_mut().enable();
    let entity_movement = |tick: i32| ServerEntityMove {
        id: EntityId(42),
        position: Vec3::new(f64::from(tick), 64.0, 0.5),
        wire_move: Some(ServerEntityWireMove::Position {
            delta: Vec3::new(0.25, 0.0, 0.0),
        }),
        velocity: Vec3::new(f64::from(tick), 0.0, 0.0),
        rotation: Rotation::ZERO,
        on_ground: true,
        send_velocity: false,
        send_head_rotation: false,
    };
    let player_movement = |tick: i32| {
        OutboundCommand::MovePlayer(PlayerEntitySnapshot {
            session_id: 100,
            entity_id: 100,
            uuid: uuid::Uuid::nil(),
            name: "moving-player".to_owned(),
            properties: Vec::new(),
            pose: PlayerPose::new(f64::from(tick), 64.0, 0.5),
        })
    };

    dispatch_visibility_command(
        &recipient,
        OutboundCommand::MoveEntityRelative(entity_movement(1)),
    );
    first_dequeued.await;
    for tick in 2..=64 {
        dispatch_visibility_command(&recipient, player_movement(tick));
        dispatch_visibility_command(
            &recipient,
            OutboundCommand::MoveEntityRelative(entity_movement(tick)),
        );
    }

    for expected in 1..=16 {
        assert!(matches!(
            rx.recv().await,
            Some(OutboundCommand::AnimatePlayer { entity_id }) if entity_id == expected
        ));
    }
    assert!(matches!(
        rx.recv().await,
        Some(OutboundCommand::MoveEntityRelative(ServerEntityMove { position, .. }))
            if position == Vec3::new(1.0, 64.0, 0.5)
    ));
    assert!(matches!(
        rx.recv().await,
        Some(OutboundCommand::MovePlayer(PlayerEntitySnapshot { pose, .. }))
            if pose.x == 64.0
    ));
    assert!(matches!(
        rx.recv().await,
        Some(OutboundCommand::MoveEntitiesRelative(movements))
            if movements.len() == 1
                && movements[0].position == Vec3::new(64.0, 64.0, 0.5)
                && matches!(
                    movements[0].wire_move,
                    Some(ServerEntityWireMove::Absolute { .. })
                )
    ));
    assert!(rx.try_recv().is_err(), "stale movement turns were queued");
    retry_completed.await;
    let snapshot = registry.pressure_snapshot();
    assert_eq!(snapshot.reliable_command_drops, 0);
    assert_eq!(snapshot.slow_client_pressure_sheds, 0);
    assert_eq!(snapshot.reliable_command_retries_in_flight, 0);
}

#[test]
fn full_reliable_channel_without_tokio_runtime_uses_blocking_worker() {
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(1);
    tx.try_send(OutboundCommand::AnimatePlayer { entity_id: 1 })
        .expect("fill recipient queue");
    let recipient = test_recipient(&registry, 72, tx);

    dispatch_visibility_command(
        &recipient,
        OutboundCommand::SystemChat {
            message: "after-full-channel".to_owned(),
        },
    );

    let (received_tx, received_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let first = rx.blocking_recv();
        let second = rx.blocking_recv();
        let _ = received_tx.send((first, second));
    });
    let (first, second) = received_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("blocking retry worker must make progress before the failure timeout");
    assert!(matches!(
        first,
        Some(OutboundCommand::AnimatePlayer { entity_id: 1 })
    ));
    assert!(matches!(
        second,
        Some(OutboundCommand::SystemChat { message }) if message == "after-full-channel"
    ));
}
