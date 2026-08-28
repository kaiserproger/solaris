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

fn face_first_hostile_towards_player(registry: &SessionRegistry) {
    let mut entities = registry.lock_entities("face test hostile toward player");
    let expected = entities.snapshots().next().expect("test hostile");
    let mut next = expected.clone();
    let dx = 0.5 - next.position.x;
    let dz = 0.5 - next.position.z;
    let yaw = dz.atan2(dx).to_degrees() as f32 - 90.0;
    next.rotation = Rotation {
        yaw,
        pitch: 0.0,
        head_yaw: yaw,
    };
    assert!(entities.replace_snapshot_if_current(expected, next));
}

fn install_hostile_target_snapshot_probe(
    registry: &SessionRegistry,
) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .hostile_target_snapshot_probe
        .lock()
        .expect("test lock poisoned") = Some(HostileCommitProbe {
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
    face_first_hostile_towards_player(&registry);
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

    let (attacks, dispatches) =
        registry.tick_hostile_attacks(&SimulationAuthority::for_test(), due_tick, BlockStateId(0));
    assert_eq!(attacks, 0, "a hostile facing away cannot deal melee damage");
    assert!(dispatches.is_empty());

    face_first_hostile_towards_player(&registry);

    for tick in [
        due_tick + HOSTILE_MELEE_PERIOD_TICKS,
        due_tick + 2 * HOSTILE_MELEE_PERIOD_TICKS,
    ] {
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
fn active_guardian_beam_keeps_idle_goal_during_common_target_update() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "GuardianIdleTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        63,
        "minecraft:guardian".to_owned(),
        Vec3::new(0.5, 64.0, 6.5),
    );
    let target_entity_id = registry
        .lock_inner("read guardian idle target entity")
        .sessions[&player]
        .entity_id;
    {
        let mut entities = registry.lock_entities("install guardian beam fixture");
        let expected = entities.snapshots().next().expect("guardian fixture");
        let mut next = expected.clone();
        next.goal = mc_entity::GoalState::Idle;
        next.retained.guardian_beam = Some(mc_entity::EntityGuardianBeamState::new(
            mc_entity::EntityGuardianBeamPhase::Warmup,
            player,
            target_entity_id,
            20,
        ));
        assert!(entities.replace_snapshot_if_current(expected, next));
    }

    let queries = registry.tick_entities_and_collect_physics_queries(2);

    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].goal_fence, mc_entity::EntityGoalFence::Idle);
    assert_eq!(queries[0].velocity.x, 0.0);
    assert_eq!(queries[0].velocity.z, 0.0);
    let guardian = &registry.persisted_entity_records()[0].snapshot;
    assert_eq!(guardian.goal, mc_entity::GoalState::Idle);
    assert!(guardian.retained.guardian_beam.is_some());
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
    face_first_hostile_towards_player(&registry);
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
    face_first_hostile_towards_player(&registry);
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
    face_first_hostile_towards_player(&registry);
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
    face_first_hostile_towards_player(&registry);
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
fn attacker_turn_between_melee_plan_and_commit_cancels_damage_and_swing() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "TurningAttackerTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_owned(),
        Vec3::new(0.5, 64.0, 1.5),
    );
    face_first_hostile_towards_player(&registry);
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
        .expect("melee attack reserves its target before the attacker fence");

    let mut entities = registry.lock_entities("turn hostile away before melee commit");
    let expected = entities.snapshots().next().expect("test hostile");
    let mut turned = expected.clone();
    turned.rotation.yaw = 0.0;
    turned.rotation.head_yaw = 0.0;
    assert!(entities.replace_snapshot_if_current(expected, turned));
    drop(entities);
    resume_tx.send(()).expect("release hostile commit");
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
    face_first_hostile_towards_player(&registry);
    let zombie = registry.persisted_entity_records()[0].snapshot.clone();
    let due_tick = due_melee_tick(&registry);
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .hostile_commit_probe
        .lock()
        .expect("test lock poisoned") = Some(HostileCommitProbe {
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
                villager_gossip_event: None,
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
    face_first_hostile_towards_player(&registry);
    let due_tick = due_melee_tick(&registry);
    let (reached_tx, reached_rx) = std::sync::mpsc::channel();
    let (resume_tx, resume_rx) = std::sync::mpsc::channel();
    *registry
        .hostile_publication_probe
        .lock()
        .expect("test lock poisoned") = Some(HostileCommitProbe {
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
fn ordinary_hostile_melee_tick_never_waits_for_session_registry() {
    let registry = Arc::new(SessionRegistry::new());
    let player = register_test_session(&registry, "DetachedHostileTick");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_owned(),
        Vec3::new(0.5, 64.0, 1.5),
    );
    face_first_hostile_towards_player(&registry);
    assert_eq!(
        registry.tick_entities_and_collect_physics_queries(1).len(),
        1
    );
    let due_tick = due_melee_tick(&registry);
    let session_guard = registry.inner.lock().expect("session registry poisoned");
    let (finished_tx, finished_rx) = std::sync::mpsc::channel();
    let attack_registry = Arc::clone(&registry);
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

    let (attacks, dispatches) = finished_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("ordinary melee tick must not wait for the session registry");
    drop(session_guard);
    attack.join().expect("hostile attack worker");

    assert_eq!(attacks, 1);
    assert!(dispatches.iter().any(|dispatch| {
        dispatch.recipient.id == player
            && matches!(dispatch.command, OutboundCommand::DamagePlayer { .. })
    }));
}

#[test]
fn hostile_tick_uses_current_loaded_selection_and_clears_without_players() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "HostileSelectionTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        54,
        "minecraft:zombie".to_owned(),
        Vec3::new(0.5, 64.0, 1.5),
    );
    let zombie = registry.persisted_entity_records()[0].snapshot.id;

    assert_eq!(
        registry.tick_entities_and_collect_physics_queries(1).len(),
        1
    );
    assert!(registry.active_hostile_entities.load().contains(&zombie));

    registry.mark_unloaded(player, &[(0, 0)]);
    assert!(
        registry
            .tick_entities_and_collect_physics_queries(2)
            .is_empty()
    );
    assert!(registry.active_hostile_entities.load().is_empty());
    registry.reset_entity_owner_requests_for_test();
    let (attacks, dispatches) = registry.tick_hostile_attacks(
        &SimulationAuthority::for_test(),
        due_melee_tick(&registry),
        BlockStateId(0),
    );
    assert_eq!(attacks, 0);
    assert!(dispatches.is_empty());
    assert_eq!(registry.entity_owner_requests_for_test(), 0);

    registry.mark_loaded(player, (0, 0));
    assert_eq!(
        registry.tick_entities_and_collect_physics_queries(3).len(),
        1
    );
    registry.unregister(player);
    assert!(registry.active_simulation_entities.load().is_empty());
    assert!(registry.active_hostile_entities.load().is_empty());
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
fn due_tnt_is_drained_in_bounded_deadline_batches() {
    let registry = SessionRegistry::new();
    for index in 0..10 {
        registry.spawn_chained_primed_tnt(
            &SimulationAuthority::for_test(),
            132,
            Vec3::new(f64::from(index) + 0.5, 64.0, 0.5),
            Vec3::ZERO,
            5,
            BlockStateId(0),
        );
    }

    assert!(
        registry
            .claim_due_primed_tnt(&SimulationAuthority::for_test(), 4)
            .is_empty()
    );
    let mut claimed = 0;
    for tick in 5..15 {
        let batch = registry.claim_due_primed_tnt(&SimulationAuthority::for_test(), tick);
        assert!(!batch.is_empty());
        assert!(batch.len() <= super::explosion_authority::EXPLOSIONS_PER_TICK);
        claimed += batch.len();
        assert!(
            registry
                .claim_due_primed_tnt(&SimulationAuthority::for_test(), tick)
                .is_empty(),
            "the per-tick budget must survive repeated owner calls"
        );
    }
    assert_eq!(claimed, 10);
    assert!(registry.primed_tnt_fuses_for_test().is_empty());
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
    {
        let inner = registry.lock_inner("verify cancelled creeper deadline cleanup");
        assert!(inner.primed_tnt_deadlines.is_empty());
        assert!(inner.primed_tnt_deadline_by_id.is_empty());
    }
    assert!(
        registry
            .claim_due_primed_tnt(&SimulationAuthority::for_test(), 40)
            .is_empty(),
        "cancelled creeper fuse must not leave a deadline entry"
    );
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
                    villager_gossip_event: None,
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

#[test]
fn warden_sonic_boom_charges_damages_and_cools_down() {
    let registry = SessionRegistry::new();
    let player = register_test_session(&registry, "WardenTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        140,
        "minecraft:warden".to_owned(),
        Vec3::new(0.5, 64.0, 8.5),
    );
    let warden_id = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:warden")
        .expect("test warden")
        .snapshot
        .id;
    let sonic_state = || {
        registry
            .persisted_entity_records()
            .into_iter()
            .find(|record| record.snapshot.id == warden_id)
            .expect("test warden remains alive")
            .snapshot
            .retained
            .warden_sonic_boom
    };

    let (attacks, charge_dispatches) =
        registry.tick_hostile_attacks(&SimulationAuthority::for_test(), 0, BlockStateId(0));
    assert_eq!(attacks, 0);
    assert_eq!(
        sonic_state(),
        Some(mc_entity::EntityWardenSonicBoomState::new(
            mc_entity::EntityWardenSonicBoomPhase::Charging,
            player,
            1,
            34,
        ))
    );
    assert!(charge_dispatches.iter().any(|dispatch| {
        matches!(
            dispatch.command,
            OutboundCommand::EntityEvent {
                entity_id,
                event_id: 62
            } if entity_id == warden_id.0
        )
    }));

    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 33, BlockStateId(0))
            .0,
        0
    );
    let (attacks, damage_dispatches) =
        registry.tick_hostile_attacks(&SimulationAuthority::for_test(), 34, BlockStateId(0));
    assert_eq!(attacks, 1);
    assert_eq!(
        sonic_state().expect("warden recovery state").phase,
        mc_entity::EntityWardenSonicBoomPhase::Recovery
    );
    assert!(damage_dispatches.iter().any(|dispatch| {
        matches!(
            dispatch.command,
            OutboundCommand::DamagePlayer { damage }
                if damage.kind == PlayerDamageKind::SonicBoom && damage.amount == 10.0
        )
    }));

    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 60, BlockStateId(0))
            .0,
        0
    );
    assert_eq!(
        sonic_state().expect("warden cooldown state").phase,
        mc_entity::EntityWardenSonicBoomPhase::Cooldown
    );
    registry.tick_hostile_attacks(&SimulationAuthority::for_test(), 100, BlockStateId(0));
    assert_eq!(sonic_state(), None);
}

#[test]
fn breeze_charges_shoots_recovers_and_cools_down_with_owned_wind_charge() {
    let registry = SessionRegistry::new();
    registry.configure_hostile_breeze_wind_charge_entity_type(Some(115));
    let player = register_test_session(&registry, "BreezeTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    let target_entity_id = {
        let inner = registry.lock_inner("read breeze target entity id");
        inner.sessions[&player].entity_id
    };
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        16,
        "minecraft:breeze".to_owned(),
        Vec3::new(0.5, 64.0, 8.5),
    );
    let breeze_id = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:breeze")
        .expect("test breeze")
        .snapshot
        .id;

    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 0, BlockStateId(0))
            .0,
        0
    );
    let breeze_state = || {
        registry
            .persisted_entity_records()
            .into_iter()
            .find(|record| record.snapshot.id == breeze_id)
            .expect("breeze remains authoritative")
            .snapshot
            .retained
            .breeze_attack
    };
    assert_eq!(
        breeze_state(),
        Some(mc_entity::EntityBreezeAttackState::new(
            mc_entity::EntityBreezeAttackPhase::Charging,
            player,
            target_entity_id,
            15,
        ))
    );
    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 14, BlockStateId(0))
            .0,
        0
    );
    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 15, BlockStateId(0))
            .0,
        1
    );
    assert_eq!(
        breeze_state(),
        Some(mc_entity::EntityBreezeAttackState::new(
            mc_entity::EntityBreezeAttackPhase::Recovery,
            player,
            target_entity_id,
            19,
        ))
    );

    let wind_charge = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:breeze_wind_charge")
        .expect("breeze wind charge spawned")
        .snapshot;
    assert_eq!(wind_charge.type_id, 115);
    let projectile = wind_charge
        .retained
        .hurting_projectile_state
        .expect("wind charge uses generic projectile kernel");
    assert_eq!(
        projectile.projectile.owner.expect("breeze owner").raw(),
        u128::from(breeze_id.0 as u32)
    );
    assert_eq!(projectile.acceleration_power, 0.0);
    assert_eq!(projectile.air_inertia, 1.0);
    assert_eq!(projectile.water_inertia, 1.0);
    let speed = (wind_charge.velocity.x * wind_charge.velocity.x
        + wind_charge.velocity.y * wind_charge.velocity.y
        + wind_charge.velocity.z * wind_charge.velocity.z)
        .sqrt();
    assert!((speed - 0.7).abs() < 1.0e-9, "wind charge speed={speed}");
    let explosion = wind_charge
        .retained
        .pending_explosion
        .expect("wind charge retains trigger explosion source");
    assert_eq!(explosion.power(), 3.0);
    assert_eq!(
        explosion.interaction,
        mc_entity::EntityExplosionInteraction::Trigger
    );
    assert!(!explosion.damage_entities);

    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 19, BlockStateId(0))
            .0,
        0
    );
    assert_eq!(
        breeze_state().expect("breeze cooldown state").phase,
        mc_entity::EntityBreezeAttackPhase::Cooldown
    );
    registry.tick_hostile_attacks(&SimulationAuthority::for_test(), 29, BlockStateId(0));
    assert_eq!(breeze_state(), None);
}

#[test]
fn ghast_charges_and_spawns_owned_large_fireball_with_mob_explosion() {
    let registry = SessionRegistry::new();
    registry.configure_hostile_fireball_entity_type(Some(112));
    let player = register_test_session(&registry, "GhastTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        45,
        "minecraft:ghast".to_owned(),
        Vec3::new(0.5, 64.0, 10.5),
    );
    let ghast_id = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:ghast")
        .expect("test ghast")
        .snapshot
        .id;

    for tick in 0..19 {
        assert_eq!(
            registry
                .tick_hostile_attacks(&SimulationAuthority::for_test(), tick, BlockStateId(0))
                .0,
            0,
            "ghast must not fire before charge tick 20"
        );
    }
    let charging = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.id == ghast_id)
        .expect("ghast remains authoritative")
        .snapshot;
    assert_eq!(
        charging.retained.ghast_attack,
        Some(mc_entity::EntityGhastAttackState::new(19))
    );

    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 19, BlockStateId(0))
            .0,
        1
    );
    let ghast = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.id == ghast_id)
        .expect("ghast remains after firing")
        .snapshot;
    assert_eq!(
        ghast.retained.ghast_attack,
        Some(mc_entity::EntityGhastAttackState::new(-40))
    );

    let fireball = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:fireball")
        .expect("large fireball spawned")
        .snapshot;
    assert_eq!(fireball.type_id, 112);
    let projectile = fireball
        .retained
        .hurting_projectile_state
        .expect("large fireball uses generic projectile kernel");
    assert_eq!(
        projectile.projectile.owner.expect("ghast owner").raw(),
        u128::from(ghast_id.0 as u32)
    );
    let explosion = fireball
        .retained
        .pending_explosion
        .expect("large fireball retains explosion source");
    assert_eq!(explosion.power(), 1.0);
    assert_eq!(
        explosion.interaction,
        mc_entity::EntityExplosionInteraction::Mob
    );
    assert!(explosion.damage_entities);
    assert_eq!(explosion.air_block_state, 0);
    assert_eq!(explosion.expires_tick, u64::MAX);
    assert!(fireball.velocity != Vec3::ZERO);
}

#[test]
fn witch_waits_sixty_ticks_and_throws_owned_slowness_potion_at_distant_player() {
    let registry = SessionRegistry::new();
    registry.configure_hostile_splash_potion_entity_type(Some(116));
    let player = register_test_session(&registry, "WitchTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        125,
        "minecraft:witch".to_owned(),
        Vec3::new(0.5, 64.0, 8.5),
    );
    let witch_id = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:witch")
        .expect("test witch")
        .snapshot
        .id;
    let target_entity_id = {
        let inner = registry.lock_inner("read witch target entity id");
        inner.sessions[&player].entity_id
    };

    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 0, BlockStateId(0))
            .0,
        0
    );
    let witch = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.id == witch_id)
        .expect("witch remains authoritative")
        .snapshot;
    assert_eq!(
        witch.retained.witch_attack,
        Some(mc_entity::EntityWitchAttackState::new(
            player,
            target_entity_id,
            60,
        ))
    );
    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 59, BlockStateId(0))
            .0,
        0
    );
    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 60, BlockStateId(0))
            .0,
        1
    );

    let potion = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:splash_potion")
        .expect("witch splash potion spawned")
        .snapshot;
    assert_eq!(potion.type_id, 116);
    assert_eq!(
        potion.retained.witch_potion,
        Some(mc_entity::EntityWitchPotionKind::Slowness)
    );
    let state = potion
        .retained
        .throwable_projectile_state
        .expect("witch potion uses throwable kernel");
    assert_eq!(
        state.projectile.owner.expect("witch owner").raw(),
        u128::from(witch_id.0 as u32)
    );
    assert!(potion.velocity != Vec3::ZERO);
}

#[test]
fn wither_primary_head_spawns_owned_skull_with_mob_explosion() {
    let registry = SessionRegistry::new();
    registry.configure_hostile_wither_skull_entity_type(Some(115));
    let player = register_test_session(&registry, "WitherTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        124,
        "minecraft:wither".to_owned(),
        Vec3::new(0.5, 64.0, 8.5),
    );
    let wither_id = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:wither")
        .expect("test wither")
        .snapshot
        .id;
    let shot_period = 40_u64;
    let phase = u64::from(wither_id.0.unsigned_abs()) % shot_period;
    let due_tick = if phase == 0 {
        shot_period
    } else {
        shot_period - phase
    };

    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), due_tick, BlockStateId(0),)
            .0,
        1
    );
    let skull = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:wither_skull")
        .expect("wither skull spawned")
        .snapshot;
    assert_eq!(skull.type_id, 115);
    let projectile = skull
        .retained
        .hurting_projectile_state
        .expect("wither skull uses generic projectile kernel");
    assert_eq!(
        projectile.projectile.owner.expect("wither owner").raw(),
        u128::from(wither_id.0 as u32)
    );
    let explosion = skull
        .retained
        .pending_explosion
        .expect("wither skull retains explosion source");
    assert_eq!(explosion.power(), 1.0);
    assert_eq!(
        explosion.interaction,
        mc_entity::EntityExplosionInteraction::Mob
    );
    assert!(explosion.damage_entities);
    assert_eq!(explosion.air_block_state, 0);
    assert_eq!(explosion.expires_tick, u64::MAX);
    assert!(skull.velocity != Vec3::ZERO);
}

#[test]
fn evoker_fangs_warm_up_spawn_line_damage_and_expire() {
    let registry = SessionRegistry::new();
    registry.configure_hostile_evoker_fangs_entity_type(Some(114));
    let player = register_test_session(&registry, "EvokerTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        39,
        "minecraft:evoker".to_owned(),
        Vec3::new(0.5, 64.0, 6.5),
    );
    let evoker_id = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:evoker")
        .expect("test evoker")
        .snapshot
        .id;

    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 0, BlockStateId(0))
            .0,
        0
    );
    let evoker = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.id == evoker_id)
        .expect("evoker remains authoritative")
        .snapshot;
    assert_eq!(
        evoker.retained.evoker_attack,
        Some(mc_entity::EntityEvokerAttackState::new(
            mc_entity::EntityEvokerAttackPhase::Warmup,
            player,
            1,
            20,
        ))
    );
    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 19, BlockStateId(0))
            .0,
        0
    );
    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 20, BlockStateId(0))
            .0,
        16
    );

    let fangs = registry
        .persisted_entity_records()
        .into_iter()
        .filter(|record| record.snapshot.type_name == "minecraft:evoker_fangs")
        .map(|record| record.snapshot)
        .collect::<Vec<_>>();
    assert_eq!(fangs.len(), 16);
    let mut delays = fangs
        .iter()
        .map(|fang| {
            fang.retained
                .evoker_fangs
                .expect("fang state")
                .warmup_delay_ticks
        })
        .collect::<Vec<_>>();
    delays.sort_unstable();
    assert_eq!(delays, (0..16).collect::<Vec<_>>());
    assert!(fangs.iter().all(|fang| {
        fang.retained
            .evoker_fangs
            .is_some_and(|state| state.owner_entity_id == evoker_id.0)
    }));

    let fang_ids = fangs.iter().map(|fang| fang.id).collect::<HashSet<_>>();
    registry.publish_active_simulation_entities_for_test(fang_ids.clone());
    let mut saw_event = false;
    let mut saw_damage = false;
    for tick in 21..=60 {
        let (_, dispatches) =
            registry.tick_hostile_attacks(&SimulationAuthority::for_test(), tick, BlockStateId(0));
        saw_event |= dispatches.iter().any(|dispatch| {
            matches!(
                dispatch.command,
                OutboundCommand::EntityEvent { event_id: 4, .. }
            )
        });
        saw_damage |= dispatches.iter().any(|dispatch| {
            matches!(
                dispatch.command,
                OutboundCommand::DamagePlayer { damage }
                    if damage.kind == PlayerDamageKind::IndirectMagic && damage.amount == 6.0
            )
        });
    }
    assert!(saw_event, "evoker fangs must broadcast spike event 4");
    assert!(saw_damage, "evoker fangs must deal 6 indirect-magic damage");
    assert!(
        fang_ids
            .iter()
            .any(|id| registry.server_entity_snapshot(*id).is_none())
    );
}

#[test]
fn shulker_ranged_attack_spawns_owned_targeted_bullet_kernel() {
    let registry = SessionRegistry::new();
    registry.configure_hostile_shulker_bullet_entity_type(Some(113));
    let player = register_test_session(&registry, "ShulkerTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    let target_entity_id = {
        let inner = registry.lock_inner("read shulker target entity id");
        inner.sessions[&player].entity_id
    };
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        81,
        "minecraft:shulker".to_owned(),
        Vec3::new(0.5, 64.0, 8.5),
    );
    let shulker_id = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:shulker")
        .expect("test shulker")
        .snapshot
        .id;

    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 0, BlockStateId(0))
            .0,
        0
    );
    let shulker = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.id == shulker_id)
        .expect("shulker remains authoritative")
        .snapshot;
    assert_eq!(
        shulker.retained.shulker_attack,
        Some(mc_entity::EntityShulkerAttackState::new(20))
    );
    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 19, BlockStateId(0))
            .0,
        0
    );
    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 20, BlockStateId(0))
            .0,
        1
    );

    let bullet = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:shulker_bullet")
        .expect("shulker bullet spawned")
        .snapshot;
    assert_eq!(bullet.type_id, 113);
    assert_eq!(
        bullet.retained.shulker_bullet,
        Some(mc_entity::EntityShulkerBulletState::new(target_entity_id))
    );
    let state = bullet
        .retained
        .hurting_projectile_state
        .expect("shulker bullet uses generic projectile kernel");
    assert_eq!(
        state.projectile.owner.expect("shulker owner").raw(),
        u128::from(shulker_id.0 as u32)
    );
    assert_eq!(state.acceleration_power, 0.0);
    assert_eq!(state.air_inertia, 1.0);
    assert_eq!(state.water_inertia, 1.0);
    {
        let entities = registry.lock_entities("inspect shulker bullet projection");
        let ids = HashSet::from([bullet.id]);
        let projection = entities
            .simulation_projections_for_ids(&ids)
            .into_iter()
            .next()
            .expect("shulker bullet projection");
        assert_eq!(
            projection.shulker_bullet_target_entity_id,
            Some(target_entity_id)
        );
    }
    assert!(
        registry
            .movement_recipients
            .load_full()
            .values()
            .any(|publication| publication.entity_id() == target_entity_id)
    );

    let queries = registry.tick_entities_and_collect_physics_queries(21);
    let query = queries
        .iter()
        .find(|query| query.id == bullet.id)
        .expect("active shulker bullet physics query");
    assert!(
        query.velocity.x * query.velocity.x
            + query.velocity.y * query.velocity.y
            + query.velocity.z * query.velocity.z
            > 0.0
    );
    assert!(
        query.velocity.z < 0.0,
        "bullet must steer toward player: {query:?}"
    );
}

#[test]
fn blaze_ranged_burst_spawns_owned_small_fireball_kernel() {
    let registry = SessionRegistry::new();
    registry.configure_hostile_small_fireball_entity_type(Some(93));
    let player = register_test_session(&registry, "BlazeTarget");
    assert!(registry.mark_loaded(player, (0, 0)).is_empty());
    registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        63,
        "minecraft:blaze".to_owned(),
        Vec3::new(0.5, 64.0, 8.5),
    );
    let blaze_id = registry
        .persisted_entity_records()
        .into_iter()
        .find(|record| record.snapshot.type_name == "minecraft:blaze")
        .expect("test blaze")
        .snapshot
        .id;

    let blaze_state = || {
        registry
            .persisted_entity_records()
            .into_iter()
            .find(|record| record.snapshot.id == blaze_id)
            .expect("test blaze remains alive")
            .snapshot
            .retained
            .blaze_attack
            .expect("visible blaze retains attack goal state")
    };

    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 0, BlockStateId(0))
            .0,
        0
    );
    assert_eq!(blaze_state(), mc_entity::EntityBlazeAttackState::new(1, 60));
    assert!(blaze_state().is_charged());

    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 59, BlockStateId(0))
            .0,
        0
    );
    assert_eq!(blaze_state().deadline_tick, 60);

    for (tick, step, deadline) in [(60, 2, 66), (66, 3, 72), (72, 4, 78)] {
        assert_eq!(
            registry
                .tick_hostile_attacks(&SimulationAuthority::for_test(), tick, BlockStateId(0))
                .0,
            1,
            "each due burst step must spawn exactly one small fireball"
        );
        assert_eq!(
            blaze_state(),
            mc_entity::EntityBlazeAttackState::new(step, deadline)
        );
    }

    assert_eq!(
        registry
            .tick_hostile_attacks(&SimulationAuthority::for_test(), 78, BlockStateId(0))
            .0,
        0
    );
    assert_eq!(
        blaze_state(),
        mc_entity::EntityBlazeAttackState::new(0, 178)
    );
    assert!(!blaze_state().is_charged());

    let fireballs = registry
        .persisted_entity_records()
        .into_iter()
        .filter(|record| record.snapshot.type_name == "minecraft:small_fireball")
        .map(|record| record.snapshot)
        .collect::<Vec<_>>();
    assert_eq!(fireballs.len(), 3, "one blaze burst contains three shots");
    let fireball = fireballs[0].clone();
    let state = fireball
        .retained
        .hurting_projectile_state
        .expect("small fireball retains authoritative projectile state");
    assert_eq!(
        state.projectile.owner.expect("blaze owner").raw(),
        u128::from(blaze_id.0 as u32)
    );
    let speed = (fireball.velocity.x * fireball.velocity.x
        + fireball.velocity.y * fireball.velocity.y
        + fireball.velocity.z * fireball.velocity.z)
        .sqrt();
    assert!(
        (speed - 0.1).abs() < 1.0e-9,
        "small fireball starts with vanilla acceleration power"
    );
    assert_eq!(state.projectile.position.x, fireball.position.x);
    assert_eq!(state.projectile.position.y, fireball.position.y);
    assert_eq!(state.projectile.position.z, fireball.position.z);
}

#[test]
fn blaze_close_melee_preserves_ranged_attack_step() {
    let charged = mc_entity::EntityBlazeAttackState::new(3, 100);
    let close_registry = SessionRegistry::new();
    let close_player = register_test_session(&close_registry, "BlazeCloseTarget");
    assert!(close_registry.mark_loaded(close_player, (0, 0)).is_empty());
    close_registry.spawn_command_entity(
        &SimulationAuthority::for_test(),
        63,
        "minecraft:blaze".to_owned(),
        Vec3::new(0.5, 64.0, 1.5),
    );
    let close_blaze = close_registry.persisted_entity_records()[0].snapshot.id;
    {
        let mut entities = close_registry.lock_entities("seed close-range blaze state");
        let expected = entities.snapshot(close_blaze).expect("close-range blaze");
        let mut next = expected.clone();
        next.retained.blaze_attack = Some(charged);
        assert!(entities.replace_snapshot_if_current(expected, next));
    }
    let _ =
        close_registry.tick_hostile_attacks(&SimulationAuthority::for_test(), 100, BlockStateId(0));
    assert_eq!(
        close_registry.persisted_entity_records()[0]
            .snapshot
            .retained
            .blaze_attack,
        Some(mc_entity::EntityBlazeAttackState::new(3, 120)),
        "close melee reuses attackTime without resetting attackStep/charged"
    );
}
