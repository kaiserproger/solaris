use super::*;

use mc_entity::{EntityStore, SpawnEntity};

fn candidates(positions: &[Vec3]) -> Vec<PlayerContactCandidate> {
    let mut store = EntityStore::new();
    positions
        .iter()
        .map(|&position| {
            let id = store.spawn(SpawnEntity::new(1, "minecraft:zombie", position));
            PlayerContactCandidate {
                snapshot: store.snapshot(id).expect("spawned contact candidate"),
                aabb: mc_physics::Aabb {
                    half_width: 0.3,
                    height: 1.8,
                },
            }
        })
        .collect()
}

fn entity_only_facts() -> PlayerEntityContactFacts {
    PlayerEntityContactFacts {
        pusher_rule: mc_physics::entity_collision_26_1_2::TeamCollisionRule::Always,
        contact_rule: mc_physics::entity_collision_26_1_2::TeamCollisionRule::Always,
        team_relationship: mc_physics::entity_collision_26_1_2::TeamRelationship::NotAllied,
        player_physics_enabled: true,
        contact_physics_enabled: true,
        passenger_of_same_vehicle: false,
        player_pushable: false,
        player_is_vehicle: false,
        contact_pushable: true,
        contact_is_vehicle: false,
        contact_is_passenger: false,
        contact_is_spectator: false,
    }
}

fn context_for(
    candidates: &[PlayerContactCandidate],
    facts: PlayerEntityContactFacts,
    max_entity_cramming: u32,
    desired_cramming_roll: u8,
) -> PlayerContactContext {
    let session_id = 7;
    let entity_ids = candidates
        .iter()
        .map(|candidate| candidate.snapshot.id)
        .collect::<Vec<_>>();
    let simulation_tick = (0..256)
        .find(|&tick| {
            deterministic_cramming_roll(session_id, tick, &entity_ids) == desired_cramming_roll
        })
        .expect("all four cramming rolls are reachable");
    PlayerContactContext {
        entity_facts: candidates
            .iter()
            .map(|candidate| (candidate.snapshot.id, facts))
            .collect(),
        max_entity_cramming,
        session_id,
        simulation_tick,
    }
}

#[test]
fn exact_player_aabb_requires_strict_overlap_on_every_axis() {
    let touching = candidates(&[Vec3::new(0.6, 64.0, 0.0)]);
    let touching_context = context_for(&touching, entity_only_facts(), 24, 1);
    assert!(
        plan_player_contacts(
            PlayerPose::new(0.0, 64.0, 0.0),
            touching,
            Some(&touching_context),
        )
        .mutations()
        .is_empty()
    );

    let above = candidates(&[Vec3::new(0.0, 65.8, 0.0)]);
    let above_context = context_for(&above, entity_only_facts(), 24, 1);
    assert!(
        plan_player_contacts(PlayerPose::new(0.0, 64.0, 0.0), above, Some(&above_context),)
            .mutations()
            .is_empty()
    );

    let overlapping = candidates(&[Vec3::new(0.599, 64.0, 0.0)]);
    let overlapping_context = context_for(&overlapping, entity_only_facts(), 24, 1);
    assert_eq!(
        plan_player_contacts(
            PlayerPose::new(0.0, 64.0, 0.0),
            overlapping,
            Some(&overlapping_context),
        )
        .mutations()
        .len(),
        1
    );
}

#[test]
fn exact_contact_adds_velocity_without_nudging_position() {
    let candidates = candidates(&[Vec3::new(0.2, 64.0, 0.0)]);
    let context = context_for(&candidates, entity_only_facts(), 24, 1);

    let plan = plan_player_contacts(PlayerPose::new(0.0, 64.0, 0.0), candidates, Some(&context));

    assert!(plan.requirements().is_empty());
    let mutation = &plan.mutations()[0];
    assert_eq!(mutation.next.position, mutation.expected.position);
    assert!(mutation.next.velocity.x > mutation.expected.velocity.x);
    assert_eq!(mutation.next.velocity.y, mutation.expected.velocity.y);
    assert_eq!(mutation.next.velocity.z, mutation.expected.velocity.z);
}

#[test]
fn exact_overlap_produces_no_velocity_mutation() {
    let candidates = candidates(&[Vec3::new(0.0, 64.0, 0.0)]);
    let context = context_for(&candidates, entity_only_facts(), 24, 1);

    let plan = plan_player_contacts(PlayerPose::new(0.0, 64.0, 0.0), candidates, Some(&context));

    assert!(plan.requirements().is_empty());
    assert!(plan.mutations().is_empty());
}

#[test]
fn team_and_shared_vehicle_suppression_prevent_both_recipients() {
    let team_candidates = candidates(&[Vec3::new(0.2, 64.0, 0.0)]);
    let mut team_facts = entity_only_facts();
    team_facts.pusher_rule = mc_physics::entity_collision_26_1_2::TeamCollisionRule::Never;
    let team_context = context_for(&team_candidates, team_facts, 24, 1);
    assert!(
        plan_player_contacts(
            PlayerPose::new(0.0, 64.0, 0.0),
            team_candidates,
            Some(&team_context),
        )
        .mutations()
        .is_empty()
    );

    let vehicle_candidates = candidates(&[Vec3::new(0.2, 64.0, 0.0)]);
    let mut vehicle_facts = entity_only_facts();
    vehicle_facts.passenger_of_same_vehicle = true;
    let vehicle_context = context_for(&vehicle_candidates, vehicle_facts, 24, 1);
    assert!(
        plan_player_contacts(
            PlayerPose::new(0.0, 64.0, 0.0),
            vehicle_candidates,
            Some(&vehicle_context),
        )
        .mutations()
        .is_empty()
    );
}

#[test]
fn unavailable_collision_facts_fail_closed_with_typed_requirements() {
    let candidates = candidates(&[Vec3::new(0.2, 64.0, 0.0), Vec3::new(-0.2, 64.0, 0.0)]);

    let plan = plan_player_contacts(PlayerPose::new(0.0, 64.0, 0.0), candidates, None);

    assert!(plan.mutations().is_empty());
    assert!(matches!(
        plan.requirements(),
        [PlayerContactRequirement::CollisionFactsUnavailable { entity_ids }]
            if entity_ids.len() == 2
    ));
}

#[test]
fn independently_eligible_player_recipient_is_a_typed_follow_up() {
    let candidates = candidates(&[Vec3::new(0.2, 64.0, 0.0)]);
    let mut facts = entity_only_facts();
    facts.player_pushable = true;
    let context = context_for(&candidates, facts, 24, 1);

    let plan = plan_player_contacts(PlayerPose::new(0.0, 64.0, 0.0), candidates, Some(&context));

    assert!(plan.mutations().is_empty());
    assert!(matches!(
        plan.requirements(),
        [PlayerContactRequirement::PlayerVelocity { impulse }] if impulse.x < 0.0
    ));
}

#[test]
fn deterministic_cramming_roll_requests_damage_before_entity_mutations() {
    let candidates = candidates(&[Vec3::new(0.2, 64.0, 0.0)]);
    let damaging = context_for(&candidates, entity_only_facts(), 1, 0);

    let rejected = plan_player_contacts(
        PlayerPose::new(0.0, 64.0, 0.0),
        candidates.clone(),
        Some(&damaging),
    );
    assert!(rejected.mutations().is_empty());
    assert_eq!(
        rejected.requirements(),
        [PlayerContactRequirement::CrammingDamage {
            amount: mc_physics::entity_collision_26_1_2::CRAMMING_DAMAGE,
        }]
    );

    let harmless = context_for(&candidates, entity_only_facts(), 1, 1);
    assert_eq!(
        plan_player_contacts(PlayerPose::new(0.0, 64.0, 0.0), candidates, Some(&harmless),)
            .mutations()
            .len(),
        1
    );
}

#[test]
fn passenger_contacts_trigger_roll_request_but_not_cramming_damage() {
    let candidates = candidates(&[Vec3::new(0.2, 64.0, 0.0)]);
    let mut facts = entity_only_facts();
    facts.contact_is_passenger = true;
    let context = context_for(&candidates, facts, 1, 0);

    let plan = plan_player_contacts(PlayerPose::new(0.0, 64.0, 0.0), candidates, Some(&context));

    assert!(plan.requirements().is_empty());
    assert_eq!(plan.mutations().len(), 1);
}

#[test]
fn cramming_roll_seed_is_stable_across_candidate_iteration_order() {
    let first = deterministic_cramming_roll(7, 99, &[EntityId(5), EntityId(2), EntityId(8)]);
    let second = deterministic_cramming_roll(7, 99, &[EntityId(8), EntityId(5), EntityId(2)]);

    assert_eq!(first, second);
    assert!(first < mc_physics::entity_collision_26_1_2::CRAMMING_ROLL_DENOMINATOR);
}
