use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;

use super::*;
use crate::login::LoggedInProfile;
use crate::play::PlayerPose;

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

fn register_test_player_state(
    registry: &SessionRegistry,
    session: SessionId,
) -> Arc<Mutex<PlayerPersistedState>> {
    let state = Arc::new(Mutex::new(PlayerPersistedState::new_default(
        PlayerPose::new(0.5, 64.0, 0.5),
    )));
    registry.register_player_persistence(session, Arc::clone(&state));
    state
}

#[test]
fn daylight_cycle_policy_change_publishes_current_clock_rate() {
    let registry = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("DaylightPolicy"),
        name: "DaylightPolicy".to_owned(),
    };
    let (outbound, mut receiver) = mpsc::channel(8);
    registry.register(
        &profile,
        (0, 0),
        2,
        HashSet::new(),
        outbound,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    registry.set_world_time(6_000);

    registry.set_daylight_cycle_enabled(false);

    assert!(matches!(
        receiver.try_recv(),
        Ok(OutboundCommand::WorldTime {
            world_time: 6_000,
            rate: 0.0,
        })
    ));
    registry.advance_world_time(20);
    assert_eq!(registry.world_time(), 6_000);
    assert_eq!(registry.simulation_tick(), 20);

    registry.set_daylight_cycle_enabled(true);

    assert!(matches!(
        receiver.try_recv(),
        Ok(OutboundCommand::WorldTime {
            world_time: 6_000,
            rate: 1.0,
        })
    ));
}

#[test]
fn owner_time_change_that_completes_sleep_publishes_morning_once() {
    let registry = SessionRegistry::new();
    let sleeper = register_test_session(&registry, "SleepOwnerTime");
    registry.set_world_time(12_542);
    assert!(matches!(
        registry.begin_sleep(sleeper),
        SleepOutcome::Waiting { .. }
    ));
    registry.advance_world_time(DEEP_SLEEP_TICKS);

    let dispatches = registry.set_world_time_core(13_000).into_dispatches();
    let morning_publications = dispatches
        .iter()
        .filter(|dispatch| {
            matches!(
                dispatch.command,
                OutboundCommand::WorldTime {
                    world_time: 24_000,
                    ..
                }
            )
        })
        .count();

    assert_eq!(morning_publications, 1);
}

#[test]
fn owner_day_change_wakes_sleeper_and_publishes_requested_time_once() {
    let registry = SessionRegistry::new();
    let sleeper = register_test_session(&registry, "SleepOwnerDay");
    registry.set_world_time(12_542);
    assert!(matches!(
        registry.begin_sleep(sleeper),
        SleepOutcome::Waiting { .. }
    ));

    let dispatches = registry.set_world_time_core(1_000).into_dispatches();
    let day_publications = dispatches
        .iter()
        .filter(|dispatch| {
            matches!(
                dispatch.command,
                OutboundCommand::WorldTime {
                    world_time: 1_000,
                    ..
                }
            )
        })
        .count();

    assert_eq!(day_publications, 1);
    assert!(
        dispatches
            .iter()
            .any(|dispatch| { matches!(dispatch.command, OutboundCommand::WakeFromBed { .. }) })
    );
}

#[test]
fn staged_wake_retains_bed_until_exact_claim_completes() {
    let registry = SessionRegistry::new();
    let sleeper = register_test_session(&registry, "SleepStagedWake");
    let bed = mc_world::BlockPos { x: 4, y: 64, z: -3 };
    registry.set_world_time(13_000);
    assert!(matches!(
        registry.begin_sleep_at(sleeper, bed),
        SleepOutcome::Waiting { .. }
    ));

    assert_eq!(registry.request_sleep_wake(sleeper), Some(bed));
    assert!(
        registry
            .claim_sleep_wake(sleeper, mc_world::BlockPos { x: 5, ..bed })
            .is_none()
    );
    let token = registry
        .claim_sleep_wake(sleeper, bed)
        .expect("exact bed claims staged wake");
    assert!(registry.claim_sleep_wake(sleeper, bed).is_none());
    assert_eq!(registry.sleeping_bed(sleeper), Some(bed));

    registry.reject_sleep_wake(token);
    assert_eq!(registry.sleeping_bed(sleeper), Some(bed));
    let retry = registry
        .claim_sleep_wake(sleeper, bed)
        .expect("rejected release remains retryable");
    let completed = registry
        .complete_sleep_wake(retry)
        .expect("exact retry completes wake");
    assert_eq!(registry.sleeping_bed(sleeper), None);
    assert!(completed.dispatches.iter().any(|dispatch| matches!(
        dispatch.command,
        OutboundCommand::PlayerEntityData { ref values, .. }
            if values.iter().any(|value| matches!(value, EntityDataValue::Pose { pose: EntityPose::Standing, .. }))
    )));
}

#[test]
fn spectator_release_failure_rolls_back_mode_and_keeps_bed() {
    let registry = SessionRegistry::new();
    let sleeper = register_test_session(&registry, "SleepSpectatorRollback");
    let persisted = register_test_player_state(&registry, sleeper);
    let bed = mc_world::BlockPos { x: 7, y: 64, z: 2 };
    registry.set_world_time(13_000);
    assert!(matches!(
        registry.begin_sleep_at(sleeper, bed),
        SleepOutcome::Waiting { .. }
    ));

    let dispatches = registry
        .commit_player_state_event(
            &SimulationAuthority::for_test(),
            sleeper,
            PlayerStateEvent::GameMode(GameMode::Spectator),
        )
        .expect("stage spectator wake");
    assert_eq!(persisted.lock().unwrap().game_mode, GameMode::Spectator);
    assert!(
        !registry
            .lock_inner("verify deferred spectator denominator")
            .spectator_sessions
            .contains(&sleeper)
    );
    assert!(
        !dispatches
            .iter()
            .any(|dispatch| matches!(dispatch.command, OutboundCommand::PlayerEntityData { .. }))
    );

    let token = registry
        .claim_sleep_wake(sleeper, bed)
        .expect("spectator wake claim");
    assert_eq!(registry.reject_sleep_wake(token), Some(GameMode::Survival));
    assert_eq!(persisted.lock().unwrap().game_mode, GameMode::Survival);
    assert_eq!(registry.sleeping_bed(sleeper), Some(bed));
}

#[test]
fn stale_spectator_wake_cannot_overwrite_newer_mode() {
    let registry = SessionRegistry::new();
    let sleeper = register_test_session(&registry, "SleepStaleSpectator");
    let persisted = register_test_player_state(&registry, sleeper);
    let bed = mc_world::BlockPos { x: 8, y: 64, z: 2 };
    registry.set_world_time(13_000);
    assert!(matches!(
        registry.begin_sleep_at(sleeper, bed),
        SleepOutcome::Waiting { .. }
    ));
    registry
        .commit_player_state_event(
            &SimulationAuthority::for_test(),
            sleeper,
            PlayerStateEvent::GameMode(GameMode::Spectator),
        )
        .expect("stage spectator wake");
    let token = registry
        .claim_sleep_wake(sleeper, bed)
        .expect("claim staged spectator wake");
    persisted.lock().unwrap().game_mode = GameMode::Creative;

    assert_eq!(registry.reject_sleep_wake(token), None);
    assert_eq!(persisted.lock().unwrap().game_mode, GameMode::Creative);
    assert_eq!(registry.sleeping_bed(sleeper), Some(bed));
}

#[test]
fn rejected_damage_does_not_stage_wake() {
    let registry = SessionRegistry::new();
    let sleeper = register_test_session(&registry, "SleepRejectedDamage");
    let persisted = register_test_player_state(&registry, sleeper);
    let bed = mc_world::BlockPos { x: 9, y: 64, z: 1 };
    registry.set_world_time(13_000);
    assert!(matches!(
        registry.begin_sleep_at(sleeper, bed),
        SleepOutcome::Waiting { .. }
    ));
    let state = persisted.lock().unwrap().clone();
    let mut stale_survival = state.survival;
    stale_survival.food -= 1;
    let mut damaged = state.survival;
    damaged.apply_damage(1.0);

    let outcome = registry.commit_player_survival(
        &SimulationAuthority::for_test(),
        sleeper,
        &PlayerSurvivalPlan {
            expected_survival: stale_survival,
            updated_survival: damaged,
            expected_inventory: state.inventory.clone(),
            updated_inventory: state.inventory,
            expected_carried_item: state.carried_item,
            expected_xp: state.xp.clone(),
            updated_xp: state.xp,
            active_shield: None,
            enchanting_table_input: None,
            item_entity_type_id: None,
            xp_orb_entity_type_id: None,
            keep_inventory: false,
            position: Vec3::new(0.5, 64.0, 0.5),
        },
    );

    assert!(matches!(
        outcome,
        Some(PlayerSurvivalCommitOutcome::Rejected(_))
    ));
    assert_eq!(registry.sleeping_bed(sleeper), Some(bed));
    assert!(registry.claim_sleep_wake(sleeper, bed).is_none());
}

#[test]
fn accepted_lethal_damage_stages_wake_and_defers_death_publication() {
    let registry = SessionRegistry::new();
    let sleeper = register_test_session(&registry, "SleepAcceptedDamage");
    let persisted = register_test_player_state(&registry, sleeper);
    let bed = mc_world::BlockPos { x: 11, y: 64, z: 1 };
    registry.set_world_time(13_000);
    assert!(matches!(
        registry.begin_sleep_at(sleeper, bed),
        SleepOutcome::Waiting { .. }
    ));
    let state = persisted.lock().unwrap().clone();
    let mut dead = state.survival;
    dead.apply_damage(SurvivalState::MAX_HEALTH);

    let outcome = registry.commit_player_survival(
        &SimulationAuthority::for_test(),
        sleeper,
        &PlayerSurvivalPlan {
            expected_survival: state.survival,
            updated_survival: dead,
            expected_inventory: state.inventory.clone(),
            updated_inventory: state.inventory,
            expected_carried_item: state.carried_item,
            expected_xp: state.xp.clone(),
            updated_xp: state.xp,
            active_shield: None,
            enchanting_table_input: None,
            item_entity_type_id: None,
            xp_orb_entity_type_id: None,
            keep_inventory: false,
            position: Vec3::new(0.5, 64.0, 0.5),
        },
    );
    let PlayerSurvivalCommitOutcome::Committed(committed) = outcome.expect("survival outcome")
    else {
        panic!("accepted lethal damage must commit");
    };

    assert!(committed.died);
    assert!(committed.dispatches.iter().all(|dispatch| matches!(
        dispatch.command,
        OutboundCommand::WakeFromBed { bed: wake_bed } if wake_bed == bed
    )));
    assert_eq!(registry.sleeping_bed(sleeper), Some(bed));
    let token = registry
        .claim_sleep_wake(sleeper, bed)
        .expect("accepted damage stages exact wake");
    let completed = registry
        .complete_sleep_wake(token)
        .expect("bed release completion removes sleeper");
    assert!(
        completed
            .dispatches
            .iter()
            .any(|dispatch| matches!(dispatch.command, OutboundCommand::PlayerEntityData { .. }))
    );
}
