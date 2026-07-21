use std::collections::HashSet;
use std::hint::black_box;
use std::time::Instant;

use mc_entity::effects_26_1_2::{EffectDamageSource, EffectId};
use mc_entity::living_26_1_2::DamageContext;
use mc_entity::runtime_26_1_2::{EffectAction, TargetKind};
use mc_entity::{
    EntityEffectOperation, EntityEffectRequest, EntityEffectResult, EntityId, EntityLifecycle, Vec3,
};
use tokio::sync::mpsc;

use super::entity_lifecycle::{
    DEATH_REMOVALS_PER_TICK, finish_one_dying_entity_locked, schedule_entity_death_locked,
};
use super::{EntityAttackOutcome, EntityKillRewards, OutboundCommand, PlayerPose, SessionRegistry};
use crate::login::LoggedInProfile;
use crate::play::simulation::SimulationAuthority;

const ATTACKS_PER_TICK: usize = 4;

fn observed_registry() -> SessionRegistry {
    let registry = SessionRegistry::new();
    let (tx, _rx) = mpsc::channel(8);
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("CombatLoadObserver"),
        name: "CombatLoadObserver".to_owned(),
    };
    let (session, _) = registry.register(
        &profile,
        (0, 0),
        2,
        HashSet::new(),
        tx,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert!(registry.mark_loaded(session, (0, 0)).is_empty());
    registry
}

fn spawn_cows(registry: &SessionRegistry, count: usize) -> Vec<EntityId> {
    (0..count)
        .map(|index| {
            let position = Vec3::new(
                1.5 + (index % 8) as f64,
                64.0,
                1.5 + ((index / 8) % 8) as f64,
            );
            registry
                .spawn_command_entity(
                    &SimulationAuthority::for_test(),
                    5,
                    "minecraft:cow".to_owned(),
                    position,
                )
                .into_iter()
                .find_map(|dispatch| match dispatch.command {
                    OutboundCommand::SpawnEntity(entity) => Some(entity.id),
                    _ => None,
                })
                .expect("loaded observer receives cow spawn")
        })
        .collect()
}

fn attack_batch(
    registry: &SessionRegistry,
    ids: &[EntityId],
    damage: f32,
    expect_lethal: bool,
) -> usize {
    ids.iter()
        .map(|&entity_id| {
            let outcome = registry
                .attack_server_entity(
                    &SimulationAuthority::for_test(),
                    entity_id,
                    damage,
                    None,
                    &EntityKillRewards::default(),
                )
                .expect("living cow accepts damage");
            assert_eq!(
                matches!(outcome, EntityAttackOutcome::Killed { .. }),
                expect_lethal
            );
            black_box(outcome.dispatches().len())
        })
        .sum()
}

#[test]
fn death_deadlines_process_only_entities_that_were_killed() {
    const DEATHS: usize = DEATH_REMOVALS_PER_TICK * 3;

    let registry = observed_registry();
    let ids = spawn_cows(&registry, DEATHS);

    assert!(
        registry
            .lock_inner("inspect empty death index")
            .dying_entity_deadlines
            .is_empty()
    );
    assert!(
        registry
            .tick_dying_entities(&SimulationAuthority::for_test(), 0)
            .is_empty()
    );

    registry.synchronize_entity_lifecycle_epoch(10);
    assert_eq!(attack_batch(&registry, &ids, 100.0, true), ids.len() * 2);
    {
        let mut inner = registry.lock_session_entities("inspect scheduled deaths");
        let duplicate = inner.entities.snapshot(ids[0]).expect("scheduled cow");
        schedule_entity_death_locked(&mut inner, &duplicate);
        schedule_entity_death_locked(&mut inner, &duplicate);
        assert_eq!(inner.dying_entity_deadlines.len(), 1);
        assert_eq!(
            inner
                .dying_entity_deadlines
                .get(&30)
                .map(|queue| queue.iter().copied().collect::<Vec<_>>()),
            Some(ids.clone())
        );
    }

    assert!(
        registry
            .tick_dying_entities(&SimulationAuthority::for_test(), 29)
            .is_empty()
    );
    let mut removals = registry.tick_dying_entities(&SimulationAuthority::for_test(), 30);
    assert_eq!(removals.len(), DEATH_REMOVALS_PER_TICK * 2);
    for tick in 31..30 + ids.len().div_ceil(DEATH_REMOVALS_PER_TICK) as u64 {
        removals.extend(registry.tick_dying_entities(&SimulationAuthority::for_test(), tick));
    }
    assert_eq!(removals.len(), ids.len() * 2);
    assert!(
        registry
            .lock_inner("inspect drained death index")
            .dying_entity_deadlines
            .is_empty()
    );
    assert!(registry.persisted_entity_records().is_empty());
}

#[test]
fn lethal_effect_damage_uses_the_same_death_deadline_index() {
    let registry = observed_registry();
    let entity_id = spawn_cows(&registry, 1)[0];
    let expected = registry
        .lock_entities("read effect target")
        .snapshot(entity_id)
        .expect("effect target");
    let (result, _) = registry.apply_server_entity_effect_request(
        &SimulationAuthority::for_test(),
        Some(expected),
        entity_id,
        EntityEffectRequest {
            operation: EntityEffectOperation::ApplyAction {
                effect_id: EffectId::new(7),
                action: EffectAction::Damage {
                    amount: 100.0,
                    source: EffectDamageSource::Magic,
                },
                damage_context: Some(DamageContext::default()),
            },
            target_kind: TargetKind::NonPlayer,
            death_remove_tick: 20,
        },
    );
    let EntityEffectResult::Applied(applied) = result else {
        panic!("lethal effect damage must commit");
    };
    assert_eq!(applied.snapshot.lifecycle, EntityLifecycle::Despawning);
    let inner = registry.lock_inner("inspect effect death deadline");
    assert_eq!(
        inner
            .dying_entity_deadlines
            .get(&20)
            .map(|queue| queue.iter().copied().collect::<Vec<_>>()),
        Some(vec![entity_id])
    );
    drop(inner);

    assert_eq!(
        registry
            .tick_dying_entities(&SimulationAuthority::for_test(), 20)
            .len(),
        2
    );
    assert!(registry.server_entity_snapshot(entity_id).is_none());
}

#[test]
fn stale_death_removal_requeues_once_after_owner_revision() {
    let registry = observed_registry();
    let entity_id = spawn_cows(&registry, 1)[0];
    registry.synchronize_entity_lifecycle_epoch(10);
    assert_eq!(attack_batch(&registry, &[entity_id], 100.0, true), 2);

    let owner = registry.entities.handle.clone();
    let mut inner = registry.lock_session_entities("inject stale death removal revision");
    let expected = inner.entities.snapshot(entity_id).expect("dying cow");
    inner.dying_entity_deadlines.clear();
    inner.dying_entity_deadline_by_id.clear();
    assert!(
        owner
            .set_position(entity_id, Vec3::new(2.5, 64.0, 2.5))
            .expect("owner position revision")
    );
    assert!(finish_one_dying_entity_locked(&mut inner, 30, expected).is_empty());
    assert_eq!(inner.dying_entity_deadline_by_id.get(&entity_id), Some(&31));
    drop(inner);

    assert_eq!(
        registry
            .tick_dying_entities(&SimulationAuthority::for_test(), 31)
            .len(),
        2
    );
    assert!(
        registry
            .tick_dying_entities(&SimulationAuthority::for_test(), 32)
            .is_empty()
    );
    assert!(registry.server_entity_snapshot(entity_id).is_none());
}

#[test]
#[ignore = "explicit O3 mob combat load benchmark"]
fn mob_combat_load_benchmark_report() {
    const ENTITIES: usize = 4_096;
    const IDLE_TICKS: u64 = 1_000;

    let registry = observed_registry();
    let ids = spawn_cows(&registry, ENTITIES);
    let mut idle_us = Vec::with_capacity(IDLE_TICKS as usize);
    for tick in 0..IDLE_TICKS {
        let started = Instant::now();
        assert!(
            registry
                .tick_dying_entities(&SimulationAuthority::for_test(), tick)
                .is_empty()
        );
        idle_us.push(started.elapsed().as_micros());
    }

    registry.synchronize_entity_lifecycle_epoch(IDLE_TICKS + 10);
    let mut lethal_us = Vec::with_capacity(ENTITIES / ATTACKS_PER_TICK);
    let mut lethal_dispatches = 0;
    for batch in ids.chunks(ATTACKS_PER_TICK) {
        let started = Instant::now();
        lethal_dispatches += attack_batch(&registry, batch, 100.0, true);
        lethal_us.push(started.elapsed().as_micros());
    }
    assert_eq!(lethal_dispatches, ENTITIES * 2);

    let remove_tick = IDLE_TICKS + 30;
    let mut cleanup_us = Vec::with_capacity(ENTITIES.div_ceil(DEATH_REMOVALS_PER_TICK));
    let mut removal_dispatches = 0;
    for offset in 0..ENTITIES.div_ceil(DEATH_REMOVALS_PER_TICK) {
        let started = Instant::now();
        removal_dispatches += registry
            .tick_dying_entities(
                &SimulationAuthority::for_test(),
                remove_tick + offset as u64,
            )
            .len();
        cleanup_us.push(started.elapsed().as_micros());
    }
    assert_eq!(removal_dispatches, ENTITIES * 2);
    assert!(registry.persisted_entity_records().is_empty());

    idle_us.sort_unstable();
    lethal_us.sort_unstable();
    cleanup_us.sort_unstable();
    let percentile = |samples: &[u128], percentile: usize| {
        samples[(samples.len() * percentile).div_ceil(100).saturating_sub(1)]
    };
    let lethal_p99_us = percentile(&lethal_us, 99);
    let cleanup_p99_us = percentile(&cleanup_us, 99);
    println!(
        "MOB_COMBAT_LOAD_BENCH entities={ENTITIES} attacks_per_tick={ATTACKS_PER_TICK} removals_per_tick={DEATH_REMOVALS_PER_TICK} idle_ticks={IDLE_TICKS} idle_p50_us={} idle_p99_us={} lethal_p50_us={} lethal_p95_us={} lethal_p99_us={lethal_p99_us} cleanup_p50_us={} cleanup_p99_us={cleanup_p99_us}",
        percentile(&idle_us, 50),
        percentile(&idle_us, 99),
        percentile(&lethal_us, 50),
        percentile(&lethal_us, 95),
        percentile(&cleanup_us, 50),
    );
    assert!(
        lethal_p99_us < 50_000,
        "sustained lethal attack batch exceeded one 50 ms tick at p99: {lethal_p99_us} us"
    );
    assert!(
        cleanup_p99_us < 50_000,
        "bounded indexed death cleanup exceeded one 50 ms tick at p99: {cleanup_p99_us} us"
    );
}
