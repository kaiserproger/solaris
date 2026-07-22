use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use mc_entity::Vec3;
use mc_world::BlockStateId;
use tokio::sync::mpsc;

use super::*;
use crate::login::LoggedInProfile;

fn register_test_session(registry: &SessionRegistry, name: &str) -> SessionId {
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid(name),
        name: name.to_owned(),
    };
    let (outbound, _receiver) = mpsc::channel(8);
    registry
        .register(
            &profile,
            (0, 0),
            2,
            HashSet::new(),
            outbound,
            PlayerPose::new(0.5, 64.0, 0.5),
        )
        .0
}

fn due_melee_tick(registry: &SessionRegistry) -> u64 {
    let entity_id = registry.persisted_entity_records()[0].snapshot.id;
    let phase = u64::from(entity_id.0.unsigned_abs()) % HOSTILE_MELEE_PERIOD_TICKS;
    if phase == 0 {
        HOSTILE_MELEE_PERIOD_TICKS
    } else {
        HOSTILE_MELEE_PERIOD_TICKS - phase
    }
}

fn install_hostile_target_snapshot_probe(
    registry: &SessionRegistry,
) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .hostile_target_snapshot_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(HostileCommitProbe {
        reached: reached_tx,
        resume: resume_rx,
    });
    (reached_rx, resume_tx)
}

#[test]
fn hostiles_ignore_dead_players() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "DeadTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_owned(),
        Vec3::new(0.5, 64.0, 1.5),
    );
    registry.mark_player_dead_for_test(player);

    for tick in 0..HOSTILE_MELEE_PERIOD_TICKS {
        let (attacks, dispatches) =
            registry.tick_hostile_attacks(&SimulationAuthority::for_test(), tick, BlockStateId(0));
        assert_eq!(attacks, 0);
        assert!(dispatches.is_empty());
    }
}

#[test]
fn stationary_live_player_is_attacked_on_each_due_turn() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "StationaryTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_owned(),
        Vec3::new(0.5, 64.0, 1.5),
    );
    let due_tick = due_melee_tick(&registry);

    for tick in [due_tick, due_tick + HOSTILE_MELEE_PERIOD_TICKS] {
        let (attacks, dispatches) =
            registry.tick_hostile_attacks(&SimulationAuthority::for_test(), tick, BlockStateId(0));
        assert_eq!(attacks, 1);
        assert!(dispatches.iter().any(|dispatch| {
            dispatch.recipient.id == player
                && matches!(dispatch.command, OutboundCommand::DamagePlayer { .. })
        }));
        assert!(dispatches.iter().any(|dispatch| {
            matches!(
                dispatch.command,
                OutboundCommand::AnimatePlayer { entity_id } if entity_id > 0
            )
        }));
    }
}

#[test]
fn zombie_faces_stationary_melee_target_without_moving() {
    let registry = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("FacingTarget"),
        name: "FacingTarget".to_owned(),
    };
    let (outbound, mut receiver) = mpsc::channel(8);
    let (player, _) = registry.register(
        &profile,
        (0, 0),
        2,
        HashSet::new(),
        outbound,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    let spawn = registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_owned(),
        Vec3::new(1.5, 64.0, 0.5),
    );
    dispatch_visibility_commands(spawn);
    assert!(matches!(
        receiver.try_recv(),
        Ok(OutboundCommand::SpawnEntity(_))
    ));

    let queries = registry.tick_entities_and_collect_physics_queries(1);

    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].velocity.x, 0.0);
    assert_eq!(queries[0].velocity.z, 0.0);
    registry.apply_entity_physics_if_current_and_dispatch(
        ENTITY_MOVE_SEND_INTERVAL_TICKS,
        &queries,
        &[EntityPhysicsStep {
            id: queries[0].id,
            position: queries[0].position,
            velocity: queries[0].velocity,
            on_ground: queries[0].on_ground,
            horizontal_collision: false,
        }],
    );
    let zombie = &registry.persisted_entity_records()[0].snapshot;
    assert!((zombie.rotation.yaw - 90.0).abs() < f32::EPSILON);
    assert_eq!(zombie.rotation.head_yaw, zombie.rotation.yaw);
    assert!(matches!(
        zombie.goal,
        mc_entity::GoalState::FollowPosition { speed: 0.0, .. }
    ));
    assert!(matches!(
        receiver.try_recv(),
        Ok(OutboundCommand::MoveEntityRelative(movement))
            if (movement.rotation.yaw - 90.0).abs() < f32::EPSILON
                && movement.send_head_rotation
    ));
}

#[test]
fn death_between_melee_plan_and_commit_cancels_damage_and_swing() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "DyingTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_owned(),
        Vec3::new(0.5, 64.0, 1.5),
    );
    let due_tick = due_melee_tick(&registry);
    let (reached_rx, resume_tx) = install_hostile_target_snapshot_probe(&registry);
    let attack_registry = Arc::clone(&registry);
    let attack = std::thread::spawn(move || {
        attack_registry.tick_hostile_attacks(
            &SimulationAuthority::for_test(),
            due_tick,
            BlockStateId(0),
        )
    });
    reached_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("melee attack reaches commit fence");

    registry.mark_player_dead_for_test(player);
    resume_tx.send(()).expect("release hostile commit");
    let (attacks, dispatches) = attack.join().expect("hostile attack worker");

    assert_eq!(attacks, 0);
    assert!(dispatches.is_empty());
}

#[test]
fn movement_out_of_range_between_melee_plan_and_commit_cancels_attack() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "EscapingTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_owned(),
        Vec3::new(0.5, 64.0, 1.5),
    );
    let due_tick = due_melee_tick(&registry);
    let (reached_rx, resume_tx) = install_hostile_target_snapshot_probe(&registry);
    let attack_registry = Arc::clone(&registry);
    let attack = std::thread::spawn(move || {
        attack_registry.tick_hostile_attacks(
            &SimulationAuthority::for_test(),
            due_tick,
            BlockStateId(0),
        )
    });
    reached_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("melee attack reaches commit fence");

    registry.update_pose(player, PlayerPose::new(100.0, 64.0, 100.0));
    resume_tx.send(()).expect("release hostile commit");
    let (attacks, dispatches) = attack.join().expect("hostile attack worker");

    assert_eq!(attacks, 0);
    assert!(dispatches.is_empty());
}

#[test]
fn spectator_transition_after_target_snapshot_cancels_melee_attack() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "SpectatorFenceTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_owned(),
        Vec3::new(0.5, 64.0, 1.5),
    );
    let due_tick = due_melee_tick(&registry);
    let (reached_rx, resume_tx) = install_hostile_target_snapshot_probe(&registry);
    let attack_registry = Arc::clone(&registry);
    let attack = std::thread::spawn(move || {
        attack_registry.tick_hostile_attacks(
            &SimulationAuthority::for_test(),
            due_tick,
            BlockStateId(0),
        )
    });
    reached_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("melee admission snapshots target state");

    {
        let mut inner = registry.lock_inner("mark spectator during melee admission");
        inner.spectator_sessions.insert(player);
        inner.publish_combat_target(player);
    }
    resume_tx.send(()).expect("release hostile target fence");
    let (attacks, dispatches) = attack.join().expect("hostile attack worker");

    assert_eq!(attacks, 0);
    assert!(dispatches.is_empty());
}

#[test]
fn unregister_after_target_snapshot_cancels_damage_and_swing() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "DisconnectingTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_owned(),
        Vec3::new(0.5, 64.0, 1.5),
    );
    let due_tick = due_melee_tick(&registry);
    let (reached_rx, resume_tx) = install_hostile_target_snapshot_probe(&registry);
    let attack_registry = Arc::clone(&registry);
    let attack = std::thread::spawn(move || {
        attack_registry.tick_hostile_attacks(
            &SimulationAuthority::for_test(),
            due_tick,
            BlockStateId(0),
        )
    });
    reached_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("melee admission snapshots target state");

    registry.unregister(player);
    resume_tx.send(()).expect("release hostile target fence");
    let (attacks, dispatches) = attack.join().expect("hostile attack worker");

    assert_eq!(attacks, 0);
    assert!(dispatches.is_empty());
}

#[test]
fn attacker_death_between_melee_plan_and_commit_cancels_damage_and_swing() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "LiveTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_owned(),
        Vec3::new(0.5, 64.0, 1.5),
    );
    let zombie = registry.persisted_entity_records()[0].snapshot.clone();
    let due_tick = due_melee_tick(&registry);
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .hostile_commit_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(HostileCommitProbe {
        reached: reached_tx,
        resume: resume_rx,
    });
    let attack_registry = Arc::clone(&registry);
    let attack = std::thread::spawn(move || {
        attack_registry.tick_hostile_attacks(
            &SimulationAuthority::for_test(),
            due_tick,
            BlockStateId(0),
        )
    });
    reached_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("melee attack reaches commit fence");

    let mut entities = registry.lock_entities("kill hostile before melee commit");
    let damage = entities
        .damage_if_current(
            zombie,
            mc_entity::EntityDamageRequest {
                amount: 100.0,
                tick: due_tick,
                death_remove_tick: due_tick + ENTITY_DEATH_TICKS,
            },
        )
        .expect("lethal hostile damage commits");
    assert!(damage.killed);
    drop(entities);
    resume_tx.send(()).expect("release hostile commit");
    let (attacks, dispatches) = attack.join().expect("hostile attack worker");

    assert_eq!(attacks, 0);
    assert!(dispatches.is_empty());
}

#[test]
fn hostile_melee_publication_finishes_while_session_registry_is_held_elsewhere() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "DetachedHostilePublication");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_owned(),
        Vec3::new(0.5, 64.0, 1.5),
    );
    let due_tick = due_melee_tick(&registry);
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .hostile_publication_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(HostileCommitProbe {
        reached: reached_tx,
        resume: resume_rx,
    });
    let attack_registry = Arc::clone(&registry);
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let attack = std::thread::spawn(move || {
        let result = attack_registry.tick_hostile_attacks(
            &SimulationAuthority::for_test(),
            due_tick,
            BlockStateId(0),
        );
        finished_tx
            .send(result)
            .expect("hostile completion receiver remains");
    });
    reached_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("hostile owner validation reaches publication boundary");

    let session_guard = registry.inner.lock().expect("session registry poisoned");
    resume_tx.send(()).expect("release hostile publication");
    let (attacks, _) = finished_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("hostile publication must not wait for the session registry");
    drop(session_guard);
    attack.join().expect("hostile attack worker");

    assert_eq!(attacks, 1);
}

#[test]
fn nearby_creeper_primes_once_and_explodes_after_thirty_ticks() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "CreeperTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        12,
        "minecraft:creeper".to_owned(),
        Vec3::new(0.5, 64.0, 2.5),
    );

    let (ignitions, dispatches) =
        registry.tick_hostile_attacks(&SimulationAuthority::for_test(), 100, BlockStateId(17));
    assert_eq!(ignitions, 1);
    assert!(dispatches.is_empty(), "creepers do not use melee damage");
    assert_eq!(registry.primed_tnt_fuses_for_test()[0].1, 130);

    let (ignitions, _) =
        registry.tick_hostile_attacks(&SimulationAuthority::for_test(), 101, BlockStateId(17));
    assert_eq!(ignitions, 0, "an active fuse is not restarted");
    assert!(
        registry
            .claim_due_primed_tnt(&SimulationAuthority::for_test(), 129)
            .is_empty()
    );

    let mut expired = registry.claim_due_primed_tnt(&SimulationAuthority::for_test(), 130);
    assert_eq!(expired.len(), 1);
    assert_eq!(expired[0].power(), 3.0);
    assert_eq!(expired[0].air, BlockStateId(17));
    let dispatches = registry.plan_expired_tnt_dispatches(
        expired.pop().unwrap(),
        0,
        &std::collections::HashMap::new(),
    );
    assert!(
        dispatches
            .iter()
            .any(|dispatch| { matches!(dispatch.command, OutboundCommand::DespawnEntity(_)) })
    );
    assert!(
        dispatches
            .iter()
            .any(|dispatch| matches!(dispatch.command, OutboundCommand::Explosion(_)))
    );
}

#[test]
fn creeper_cancels_its_fuse_when_the_player_gets_clear() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "CreeperEscape");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        12,
        "minecraft:creeper".to_owned(),
        Vec3::new(0.5, 64.0, 2.5),
    );
    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 10, BlockStateId(0),)
            .0,
        1
    );

    registry.update_pose(player, PlayerPose::new(20.5, 64.0, 0.5));
    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 11, BlockStateId(0),)
            .0,
        0
    );
    assert!(registry.primed_tnt_fuses_for_test().is_empty());
    assert_eq!(registry.persisted_entity_records().len(), 1);
}

#[test]
fn creeper_does_not_prime_at_the_exclusive_three_block_boundary() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "CreeperBoundary");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        12,
        "minecraft:creeper".to_owned(),
        Vec3::new(0.5, 64.0, 3.5),
    );

    let (ignitions, dispatches) =
        registry.tick_hostile_attacks(&SimulationAuthority::for_test(), 10, BlockStateId(0));
    assert_eq!(ignitions, 0);
    assert!(dispatches.is_empty());
    assert!(registry.primed_tnt_fuses_for_test().is_empty());
}

#[test]
fn swelling_creeper_stops_navigation_until_its_fuse_clears() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "CreeperNavigation");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        12,
        "minecraft:creeper".to_owned(),
        Vec3::new(0.5, 64.0, 2.5),
    );
    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 10, BlockStateId(0),)
            .0,
        1
    );
    registry.update_pose(player, PlayerPose::new(0.5, 64.0, 6.5));

    let queries = registry.tick_entities_and_collect_physics_queries(11);

    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].velocity.x, 0.0);
    assert_eq!(queries[0].velocity.z, 0.0);
    assert_eq!(
        registry.persisted_entity_records()[0].snapshot.goal,
        mc_entity::GoalState::Idle
    );
}

#[test]
fn despawning_creeper_cannot_reach_fuse_expiry() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "CreeperKilled");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        12,
        "minecraft:creeper".to_owned(),
        Vec3::new(0.5, 64.0, 2.5),
    );
    registry.tick_hostile_attacks(&SimulationAuthority::for_test(), 100, BlockStateId(0));
    {
        let mut entities = registry.lock_entities("mark creeper despawning");
        let expected = entities.snapshots().next().unwrap();
        let outcome = entities
            .damage_if_current(
                expected,
                mc_entity::EntityDamageRequest {
                    amount: 100.0,
                    tick: 101,
                    death_remove_tick: 121,
                },
            )
            .expect("lethal creeper damage commits");
        assert!(outcome.killed);
    }

    assert!(
        registry
            .claim_due_primed_tnt(&SimulationAuthority::for_test(), 130)
            .is_empty()
    );
    assert_eq!(registry.persisted_entity_records().len(), 1);
}
