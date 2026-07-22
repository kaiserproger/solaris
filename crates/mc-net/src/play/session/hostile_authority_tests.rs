use std::collections::HashSet;

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
