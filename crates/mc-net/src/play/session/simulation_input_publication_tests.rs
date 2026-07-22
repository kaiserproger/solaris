use super::*;
use std::time::Duration;

fn register_test_session(registry: &SessionRegistry, name: &str) -> SessionId {
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_owned(),
    };
    let (tx, _rx) = mpsc::channel(16);
    registry
        .register(
            &profile,
            (0, 0),
            2,
            HashSet::new(),
            tx,
            PlayerPose::new(0.5, 64.0, 0.5),
        )
        .0
}

#[test]
fn ordinary_goal_tick_never_waits_for_session_registry() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "DetachedGoalTick");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        11,
        "minecraft:cow".to_owned(),
        Vec3::new(1.5, 64.0, 1.5),
    );

    let session_guard = registry.inner.lock().expect("session registry poisoned");
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let tick_registry = Arc::clone(&registry);
    let tick = std::thread::spawn(move || {
        finished_tx
            .send(tick_registry.tick_entities_and_collect_physics_queries(1))
            .expect("goal tick receiver remains");
    });

    let queries = finished_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("ordinary goal tick must not wait for the session registry");
    drop(session_guard);
    tick.join().expect("goal tick worker");
    assert_eq!(queries.len(), 1);
}

#[test]
fn chunk_and_entity_publications_follow_exact_membership() {
    let inputs = SimulationInputPublication::default();
    let entity = EntityId(7);
    let old_chunk = (0, 0);
    let new_chunk = (8, -3);

    inputs.insert_active_chunk(old_chunk);
    inputs.track_entity(old_chunk, entity);
    assert_eq!(inputs.entity_chunk(entity), Some(old_chunk));
    assert_eq!(inputs.tracked_chunk_count(), 1);
    assert_eq!(
        inputs.entity_candidates(inputs.active_chunks().as_ref()),
        HashSet::from([entity])
    );

    assert_eq!(inputs.move_entity(entity, new_chunk), Some(old_chunk));
    assert_eq!(inputs.entity_chunk(entity), Some(new_chunk));
    assert!(inputs.entities_in_chunk(old_chunk).is_none());
    assert!(
        inputs
            .entity_candidates(inputs.active_chunks().as_ref())
            .is_empty()
    );
    inputs.insert_active_chunk(new_chunk);
    assert_eq!(
        inputs.entity_candidates(inputs.active_chunks().as_ref()),
        HashSet::from([entity])
    );

    inputs.remove_active_chunk(old_chunk);
    inputs.untrack_entity(entity);
    assert_eq!(inputs.entity_chunk(entity), None);
    assert!(inputs.all_entity_ids().is_empty());
    assert!(
        inputs
            .entity_candidates(inputs.active_chunks().as_ref())
            .is_empty()
    );
}

#[test]
fn terrain_pathing_publication_applies_batched_add_and_remove() {
    let inputs = SimulationInputPublication::default();
    let first = EntityId(3);
    let second = EntityId(5);

    inputs.insert_terrain_pathing([first, second, first]);
    assert_eq!(
        inputs.terrain_pathing_entities().as_ref(),
        &HashSet::from([first, second])
    );
    inputs.remove_terrain_pathing([first, EntityId(99)]);
    assert_eq!(
        inputs.terrain_pathing_entities().as_ref(),
        &HashSet::from([second])
    );
}

#[test]
fn active_chunk_publication_follows_shared_session_refcounts() {
    let registry = SessionRegistry::new();
    let first = register_test_session(&registry, "FirstChunkObserver");
    let second = register_test_session(&registry, "SecondChunkObserver");
    let chunk = (3, -2);

    registry.mark_loaded(first, chunk);
    registry.mark_loaded(second, chunk);
    assert!(registry.simulation_inputs.active_chunks().contains(&chunk));

    registry.mark_unloaded(first, &[chunk]);
    assert!(registry.simulation_inputs.active_chunks().contains(&chunk));
    registry.mark_unloaded(second, &[chunk]);
    assert!(!registry.simulation_inputs.active_chunks().contains(&chunk));
}

#[test]
fn spectator_pose_remains_a_goal_input_without_becoming_a_combat_target() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "SpectatorGoalInput");
    {
        let mut inner = registry.inner.lock().expect("session registry poisoned");
        inner.spectator_sessions.insert(player);
        inner.publish_combat_target(player);
    }

    let recipients = registry.movement_recipients.load_full();
    let target = *recipients
        .get(&player)
        .expect("registered session publication")
        .combat_target();
    assert!(target.is_alive());
    assert!(!target.is_targetable());
}

#[test]
fn cross_shard_moves_never_disappear_from_a_concurrent_snapshot() {
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicBool, Ordering};

    let inputs = Arc::new(SimulationInputPublication::default());
    let entity = EntityId(17);
    let first_chunk = (0, 0);
    let second_chunk = (8, -3);
    inputs.insert_active_chunk(first_chunk);
    inputs.insert_active_chunk(second_chunk);
    inputs.track_entity(first_chunk, entity);

    let start = Arc::new(Barrier::new(2));
    let finished = Arc::new(AtomicBool::new(false));
    let writer_inputs = Arc::clone(&inputs);
    let writer_start = Arc::clone(&start);
    let writer_finished = Arc::clone(&finished);
    let writer = std::thread::spawn(move || {
        writer_start.wait();
        let mut destination = second_chunk;
        for _ in 0..10_000 {
            writer_inputs.move_entity(entity, destination);
            destination = if destination == first_chunk {
                second_chunk
            } else {
                first_chunk
            };
        }
        writer_finished.store(true, Ordering::Release);
    });

    start.wait();
    while !finished.load(Ordering::Acquire) {
        let (_, candidates) = inputs.active_entity_candidates();
        assert!(candidates.contains(&entity));
    }
    writer.join().expect("routing writer");
    inputs.untrack_entity(entity);
    assert!(inputs.entities_in_chunk(first_chunk).is_none());
    assert!(inputs.entities_in_chunk(second_chunk).is_none());
}
