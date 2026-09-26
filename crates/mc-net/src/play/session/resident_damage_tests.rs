//! A resident melee batch commits on the region owner lane while the session
//! lock is released. The committed health must still reach the session
//! projection when the victim's motion advanced while that batch ran.

use mc_entity::{EntityDamageRequest, GoalState, Vec3};

use super::resident_orders::publish_committed_resident_damage_locked;
use super::*;

const RESIDENT_TICK: u64 = 7;

/// Repeat the documented interleaving through the production owner paths:
/// commit the damage, let the victim's motion change, then run the publication
/// pass the resident-melee caller runs.
fn commit_then_move(amount: f32) -> (SessionRegistry, EntityId, mc_entity::EntityDamage) {
    let registry = SessionRegistry::new();
    let victim = registry.spawn_script_villager_for_test(Vec3::new(4.5, 64.0, 4.5));
    let before = registry
        .snapshot_entity_for_test(victim)
        .expect("spawned victim");
    let damage = registry
        .lock_entities("commit resident damage fixture")
        .damage_if_current(
            before,
            EntityDamageRequest {
                amount,
                tick: RESIDENT_TICK,
                death_remove_tick: RESIDENT_TICK + 20,
                villager_gossip_event: None,
            },
        )
        .expect("committed damage");
    let committed = registry
        .snapshot_entity_for_test(victim)
        .expect("committed victim");
    let mut moved = committed.clone();
    moved.position = Vec3::new(6.5, 64.0, 4.5);
    moved.velocity = Vec3::new(0.2, 0.0, 0.0);
    moved.goal = GoalState::FollowPosition {
        target: Vec3::new(9.5, 64.0, 4.5),
        speed: 1.0,
    };
    assert!(
        registry
            .lock_entities("move committed victim")
            .replace_snapshot_if_current(committed, moved),
        "the victim moved after its committed damage"
    );
    (registry, victim, damage)
}

fn publish(registry: &SessionRegistry, damage: mc_entity::EntityDamage) {
    let mut inner = registry.lock_session_entities("publish committed resident damage");
    let _ = publish_committed_resident_damage_locked(&mut inner, &[damage]);
}

#[test]
fn a_committed_lethal_resident_hit_publishes_after_the_victim_moved() {
    let (registry, victim, damage) = commit_then_move(20.0);
    assert!(damage.killed);

    publish(&registry, damage);

    let published = registry.published_entity_health_for_test(victim);
    assert_eq!(
        published,
        Some(0.0),
        "the committed death reaches the session projection after the victim moved"
    );
    assert_eq!(
        published,
        registry
            .snapshot_entity_for_test(victim)
            .map(|live| live.health),
        "the published health matches the authoritative health"
    );
}

#[test]
fn a_committed_resident_hit_publishes_after_the_victim_moved() {
    let (registry, victim, damage) = commit_then_move(8.0);
    assert!(!damage.killed);

    publish(&registry, damage);

    assert_eq!(
        registry.published_entity_health_for_test(victim),
        Some(12.0),
        "the committed damage reaches the session projection after the victim moved"
    );
}
