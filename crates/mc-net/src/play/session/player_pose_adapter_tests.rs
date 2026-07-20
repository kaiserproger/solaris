use super::*;

use std::collections::HashMap;

use mc_entity::{EntityLifecycle, SpawnEntity, Vec3};
use mc_physics::entity_collision_26_1_2::{TeamCollisionRule, TeamRelationship};

use crate::play::PlayerPose;

use crate::play::session::player_pose_authority::{
    PlayerContactCandidate, PlayerContactContext, PlayerEntityContactFacts, plan_player_contacts,
};

fn entity_only_facts() -> PlayerEntityContactFacts {
    PlayerEntityContactFacts {
        pusher_rule: TeamCollisionRule::Always,
        contact_rule: TeamCollisionRule::Always,
        team_relationship: TeamRelationship::NotAllied,
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

fn contact_plan(
    snapshots: Vec<mc_entity::EntitySnapshot>,
    facts: PlayerEntityContactFacts,
    max_entity_cramming: u32,
    desired_cramming_roll: u8,
) -> PlayerBodyPushes {
    let session_id = 7;
    let entity_ids = snapshots
        .iter()
        .map(|snapshot| snapshot.id)
        .collect::<Vec<_>>();
    let simulation_tick = (0..256)
        .find(|&tick| {
            crate::play::session::player_pose_authority::deterministic_cramming_roll(
                session_id,
                tick,
                &entity_ids,
            ) == desired_cramming_roll
        })
        .expect("all four cramming rolls are reachable");
    let context = PlayerContactContext {
        entity_facts: snapshots
            .iter()
            .map(|snapshot| (snapshot.id, facts))
            .collect::<HashMap<_, _>>(),
        max_entity_cramming,
        session_id,
        simulation_tick,
    };
    let candidates = snapshots
        .into_iter()
        .map(|snapshot| PlayerContactCandidate {
            snapshot,
            aabb: mc_physics::Aabb {
                half_width: 0.3,
                height: 1.8,
            },
        })
        .collect();
    plan_player_contacts(PlayerPose::new(0.0, 64.0, 0.0), candidates, Some(&context))
}

fn spawn_contacts(
    registry: &SessionRegistry,
    positions: &[Vec3],
) -> Vec<mc_entity::EntitySnapshot> {
    let mut entities = registry.lock_entities("spawn player contact test entities");
    positions
        .iter()
        .map(|&position| {
            let id = entities.spawn(SpawnEntity::new(1, "minecraft:zombie", position));
            entities.snapshot(id).expect("spawned contact entity")
        })
        .collect()
}

#[test]
fn owner_cas_commits_contact_velocity_without_changing_position() {
    let registry = SessionRegistry::new();
    let expected = spawn_contacts(&registry, &[Vec3::new(0.2, 64.0, 0.0)])
        .pop()
        .expect("one entity");
    let plan = contact_plan(vec![expected.clone()], entity_only_facts(), 24, 1);

    let committed = {
        let mut entities = registry.lock_entities("commit exact player contact");
        commit_player_body_pushes_locked(&mut entities, plan)
    };

    let PlayerBodyCommit::Committed(committed) = committed else {
        panic!("current owner snapshot must accept exact contact");
    };
    assert_eq!(committed.len(), 1);
    assert_eq!(committed[0].position, expected.position);
    assert!(committed[0].velocity.x > expected.velocity.x);
    let current = registry
        .lock_entities("read committed contact from owner")
        .snapshot(expected.id)
        .expect("committed entity remains");
    assert_eq!(current.position, expected.position);
    assert_eq!(current.velocity, committed[0].velocity);
}

#[test]
fn stale_member_rejects_the_whole_contact_batch_without_partial_velocity() {
    let registry = SessionRegistry::new();
    let expected = spawn_contacts(
        &registry,
        &[Vec3::new(0.2, 64.0, 0.0), Vec3::new(-0.2, 64.0, 0.0)],
    );
    let plan = contact_plan(expected.clone(), entity_only_facts(), 24, 1);
    let newer_velocity = Vec3::new(0.0, 0.0, 0.25);
    {
        let mut entities = registry.lock_entities("make one contact member stale");
        assert!(entities.set_velocity(expected[1].id, newer_velocity));
    }

    let result = {
        let mut entities = registry.lock_entities("reject stale contact batch");
        commit_player_body_pushes_locked(&mut entities, plan)
    };

    assert!(matches!(result, PlayerBodyCommit::Rejected));
    let entities = registry.lock_entities("read entities after stale contact rejection");
    let first = entities
        .snapshot(expected[0].id)
        .expect("first entity remains");
    let second = entities
        .snapshot(expected[1].id)
        .expect("second entity remains");
    assert_eq!(first.velocity, expected[0].velocity);
    assert_eq!(second.velocity, newer_velocity);
}

#[test]
fn missing_member_rejects_the_whole_contact_batch() {
    let registry = SessionRegistry::new();
    let current = spawn_contacts(&registry, &[Vec3::new(0.2, 64.0, 0.0)])
        .pop()
        .expect("one entity");
    let mut missing = current.clone();
    missing.id = mc_entity::EntityId(current.id.0 + 1_000);
    missing.position = Vec3::new(-0.2, 64.0, 0.0);
    let plan = contact_plan(vec![current.clone(), missing], entity_only_facts(), 24, 1);

    let result = {
        let mut entities = registry.lock_entities("reject missing contact batch");
        commit_player_body_pushes_locked(&mut entities, plan)
    };

    assert!(matches!(result, PlayerBodyCommit::Rejected));
    let first = registry
        .lock_entities("read entity after missing contact rejection")
        .snapshot(current.id)
        .expect("first entity remains");
    assert_eq!(first.velocity, current.velocity);
    assert_eq!(first.lifecycle, EntityLifecycle::Alive);
}

#[test]
fn disabled_physics_and_rejected_cramming_follow_up_never_reach_owner_mutation() {
    let registry = SessionRegistry::new();
    let expected = spawn_contacts(&registry, &[Vec3::new(0.2, 64.0, 0.0)])
        .pop()
        .expect("one entity");

    let mut disabled = entity_only_facts();
    disabled.contact_physics_enabled = false;
    let disabled_plan = contact_plan(vec![expected.clone()], disabled, 24, 1);
    let disabled_result = {
        let mut entities = registry.lock_entities("skip disabled contact physics");
        commit_player_body_pushes_locked(&mut entities, disabled_plan)
    };
    assert!(matches!(disabled_result, PlayerBodyCommit::Committed(ref value) if value.is_empty()));

    let cramming_plan = contact_plan(vec![expected.clone()], entity_only_facts(), 1, 0);
    let cramming_result = {
        let mut entities = registry.lock_entities("reject unowned cramming damage");
        commit_player_body_pushes_locked(&mut entities, cramming_plan)
    };
    assert!(matches!(
        cramming_result,
        PlayerBodyCommit::FollowUp(requirements)
            if requirements == [crate::play::session::player_pose_authority::PlayerContactRequirement::CrammingDamage {
                amount: mc_physics::entity_collision_26_1_2::CRAMMING_DAMAGE,
            }]
    ));

    let current = registry
        .lock_entities("read entity after disabled and cramming contacts")
        .snapshot(expected.id)
        .expect("contact entity remains");
    assert_eq!(current.position, expected.position);
    assert_eq!(current.velocity, expected.velocity);
}

#[test]
fn incomplete_current_batch_rejects_all_publication() {
    let registry = SessionRegistry::new();
    let expected = spawn_contacts(
        &registry,
        &[Vec3::new(0.2, 64.0, 0.0), Vec3::new(-0.2, 64.0, 0.0)],
    );
    let only_one = vec![server_entity_snapshot_from(expected[0].clone())];

    assert!(complete_publication_batch(expected.len(), only_one).is_none());
}
