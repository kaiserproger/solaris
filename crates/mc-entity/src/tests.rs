use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use super::*;

#[derive(Debug)]
struct TestPathingProbe {
    default: PathingProbeResult,
    overrides: HashMap<(i64, i64, i64), PathingProbeResult>,
}

impl TestPathingProbe {
    fn new(default: PathingProbeResult) -> Self {
        Self {
            default,
            overrides: HashMap::new(),
        }
    }

    fn with(mut self, x: f64, y: f64, z: f64, result: PathingProbeResult) -> Self {
        self.overrides
            .insert(pathing_test_key(Vec3::new(x, y, z)), result);
        self
    }
}

impl PathingProbe for TestPathingProbe {
    fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
        let key = pathing_test_key(position);
        self.overrides.get(&key).copied().unwrap_or(self.default)
    }
}

fn pathing_test_key(position: Vec3) -> (i64, i64, i64) {
    let quantize = |value: f64| (value * 1_000_000.0).round() as i64;
    (
        quantize(position.x),
        quantize(position.y),
        quantize(position.z),
    )
}

fn cow(position: Vec3) -> SpawnEntity {
    SpawnEntity::new(144, "minecraft:cow", position)
}

fn vehicle_snapshot(
    id: EntityId,
    lifecycle: EntityLifecycle,
    passenger: Option<EntityId>,
) -> EntitySnapshot {
    EntitySnapshot {
        id,
        uuid: deterministic_uuid(id),
        type_id: 15,
        type_name: "minecraft:oak_boat".to_owned(),
        position: Vec3::new(0.0, 63.0, 0.0),
        rotation: Rotation::ZERO,
        velocity: Vec3::ZERO,
        on_ground: true,
        item_stack: None,
        experience_value: None,
        block_state: None,
        lifecycle,
        health: if lifecycle == EntityLifecycle::Alive {
            20.0
        } else {
            0.0
        },
        attributes: AttributeSet::new(),
        goal: GoalState::Idle,
        vehicle: Some(VehicleState {
            kind: VehicleKind::Boat,
            passenger,
        }),
        animal: None,
        retained: EntityRetainedState::default(),
    }
}

#[test]
fn spawn_assigns_stable_dense_ids() {
    let mut store = EntityStore::new();
    let a = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));
    let b = store.spawn(cow(Vec3::new(2.0, 64.0, 2.0)));

    assert_eq!(a, EntityId(1));
    assert_eq!(b, EntityId(2));
    assert_eq!(store.len(), 2);
    assert!(store.contains(a));
    assert_eq!(
        store.snapshot(b).unwrap().position,
        Vec3::new(2.0, 64.0, 2.0)
    );
}

#[test]
fn insert_snapshot_rejects_duplicate_uuid_without_partial_insert() {
    let mut store = EntityStore::new();
    let id = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));
    let original = store.snapshot(id).unwrap();
    let mut duplicate = original.clone();
    duplicate.id = EntityId(99);
    duplicate.position = Vec3::new(9.0, 64.0, 9.0);

    assert!(!store.insert_snapshot(duplicate));
    assert_eq!(store.len(), 1);
    assert_eq!(store.snapshot(id), Some(original));
    assert_eq!(store.snapshot(EntityId(99)), None);
}

#[test]
fn non_finite_kinematics_are_rejected_without_mutation() {
    let mut store = EntityStore::new();
    let id = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));
    let original = store.snapshot(id).unwrap();

    assert!(!store.set_position(id, Vec3::new(f64::NAN, 64.0, 1.0)));
    assert!(!store.set_velocity(id, Vec3::new(0.0, f64::INFINITY, 0.0)));
    assert_eq!(
        store.apply_kinematics([EntityKinematics {
            id,
            position: Vec3::new(2.0, 64.0, 2.0),
            rotation: Rotation {
                yaw: f32::NAN,
                pitch: 0.0,
                head_yaw: 0.0,
            },
            velocity: Vec3::ZERO,
            on_ground: true,
        }]),
        0
    );
    assert_eq!(store.snapshot(id), Some(original));
}

#[test]
#[should_panic(expected = "entity runtime id space exhausted")]
fn allocate_id_panics_on_runtime_id_overflow() {
    let mut store = EntityStore::with_next_id(i32::MAX);
    let _ = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
}

#[test]
fn allocate_id_normalizes_negative_seed() {
    let mut store = EntityStore::with_next_id(-5);
    let id = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));

    assert_eq!(id, EntityId(1));
}

#[test]
fn item_stack_payload_round_trips_through_snapshots() {
    let mut store = EntityStore::new();
    let mut item = SpawnEntity::new(1, "minecraft:item", Vec3::new(0.5, 64.5, 0.5));
    item.item_stack = Some(EntityItemStack::new(42, 3));

    let id = store.spawn(item);

    assert_eq!(
        store.snapshot(id).and_then(|snapshot| snapshot.item_stack),
        Some(EntityItemStack::new(42, 3))
    );
    assert!(store.set_item_stack(id, Some(EntityItemStack::new(42, 1))));
    assert_eq!(
        store.snapshot(id).and_then(|snapshot| snapshot.item_stack),
        Some(EntityItemStack::new(42, 1))
    );
}

#[test]
fn ordinary_store_routes_every_family_to_ecs_runtime() {
    let mut store = EntityStore::new();
    let mut item = SpawnEntity::new(1, "minecraft:item", Vec3::new(0.5, 64.5, 0.5));
    item.item_stack = Some(EntityItemStack::new(42, 3));
    let item_id = store.spawn(item);
    let mut xp = SpawnEntity::new(2, "minecraft:experience_orb", Vec3::new(1.5, 64.5, 0.5));
    xp.experience_value = Some(7);
    let xp_id = store.spawn(xp);
    let cow_id = store.spawn(cow(Vec3::new(2.5, 64.0, 0.5)));

    assert_eq!(store.len(), 3);
    assert!(store.contains(item_id));
    assert!(store.contains(xp_id));
    assert!(store.contains(cow_id));
    assert!(store.motion_state(item_id).unwrap().is_item);
    assert!(!store.motion_state(item_id).unwrap().is_experience);
    assert!(store.motion_state(xp_id).unwrap().is_experience);
    assert!(!store.motion_state(xp_id).unwrap().is_item);
    assert_eq!(
        store.snapshot(item_id).unwrap().item_stack,
        Some(EntityItemStack::new(42, 3))
    );
    assert_eq!(store.snapshot(xp_id).unwrap().experience_value, Some(7));
    assert_eq!(
        store.apply_kinematics([EntityKinematics {
            id: item_id,
            position: Vec3::new(0.75, 64.5, 0.5),
            rotation: Rotation::ZERO,
            velocity: Vec3::ZERO,
            on_ground: true,
        }]),
        1
    );
    assert_eq!(
        store.snapshot(item_id).unwrap().position,
        Vec3::new(0.75, 64.5, 0.5)
    );
    assert_eq!(store.snapshots().count(), 3);
}

#[test]
fn simulation_views_enumerate_sole_ecs_authority_once() {
    let mut store = EntityStore::new();
    let first_id = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));
    let second_id = store.spawn(cow(Vec3::new(2.0, 64.0, 2.0)));

    let mut seen = Vec::new();
    store.visit_simulation_entities(|entity| {
        seen.push((entity.id, entity.type_name.to_owned(), entity.position));
    });
    seen.sort_unstable_by_key(|entity| entity.0);

    assert_eq!(
        seen,
        vec![
            (
                first_id,
                "minecraft:cow".to_owned(),
                Vec3::new(1.0, 64.0, 1.0)
            ),
            (
                second_id,
                "minecraft:cow".to_owned(),
                Vec3::new(2.0, 64.0, 2.0)
            ),
        ]
    );
}

#[test]
fn simulation_views_for_ids_only_visit_requested_entities_in_id_order() {
    let mut store = EntityStore::new();
    let first_id = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));
    let skipped_id = store.spawn(cow(Vec3::new(2.0, 64.0, 2.0)));
    let last_id = store.spawn(cow(Vec3::new(3.0, 64.0, 3.0)));

    let mut seen = Vec::new();
    store.visit_simulation_entities_for_ids(
        &HashSet::from([last_id, EntityId(99_999), first_id]),
        |entity| seen.push(entity.id),
    );

    assert_eq!(seen, vec![first_id, last_id]);
    assert!(!seen.contains(&skipped_id));
}

#[test]
fn alive_kinematics_for_ids_skips_despawning_entities() {
    let mut store = EntityStore::new();
    let first_id = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));
    let second_id = store.spawn(cow(Vec3::new(2.0, 64.0, 2.0)));
    let despawning_id = store.spawn(cow(Vec3::new(3.0, 64.0, 3.0)));
    assert!(store.mark_despawning(despawning_id));

    let mut states =
        store.alive_kinematics_for_ids(&HashSet::from([first_id, second_id, despawning_id]));
    states.sort_unstable_by_key(|state| state.id);

    assert_eq!(
        states,
        vec![
            EntityKinematics {
                id: first_id,
                position: Vec3::new(1.0, 64.0, 1.0),
                rotation: Rotation::ZERO,
                velocity: Vec3::ZERO,
                on_ground: true,
            },
            EntityKinematics {
                id: second_id,
                position: Vec3::new(2.0, 64.0, 2.0),
                rotation: Rotation::ZERO,
                velocity: Vec3::ZERO,
                on_ground: true,
            },
        ]
    );
}

#[test]
fn projectile_and_falling_block_families_round_trip_through_ecs_runtime() {
    let mut store = EntityStore::new();
    let arrow_id = store.spawn(SpawnEntity::new(
        1,
        "minecraft:arrow",
        Vec3::new(0.5, 66.0, 0.5),
    ));
    let mut falling = SpawnEntity::new(2, "minecraft:falling_block", Vec3::new(1.5, 70.0, 0.5));
    falling.block_state = Some(91);
    let falling_id = store.spawn(falling);
    store.spawn(cow(Vec3::new(2.5, 64.0, 0.5)));

    assert_eq!(store.len(), 3);
    assert_eq!(
        store.snapshot(arrow_id).unwrap().type_name,
        "minecraft:arrow"
    );
    assert_eq!(store.snapshot(falling_id).unwrap().block_state, Some(91));
}

#[test]
fn ordinary_passive_mob_runs_goal_tick_in_ecs() {
    let mut store = EntityStore::new();
    let mut entity = cow(Vec3::new(2.5, 64.0, 0.5));
    entity.goal = GoalState::Wander {
        speed: 0.2,
        period_ticks: 20,
    };
    let id = store.spawn(entity);

    let stats = store.tick_goals_with_stats(20);

    assert_eq!(stats.alive_entities, 1);
    assert_eq!(stats.decisions_applied, 1);
    assert_ne!(store.snapshot(id).unwrap().velocity, Vec3::ZERO);

    let snapshot = store.snapshot(id).unwrap();
    let mut restored = EntityStore::new();
    assert!(restored.insert_snapshot(snapshot.clone()));
    assert_eq!(restored.snapshot(id), Some(snapshot));
}

#[test]
fn batch_spawn_inserts_all_entities_into_ecs_runtime() {
    let mut store = EntityStore::new();

    let ids = store.spawn_batch([
        cow(Vec3::new(0.5, 64.0, 0.5)),
        cow(Vec3::new(1.5, 64.0, 0.5)),
        cow(Vec3::new(2.5, 64.0, 0.5)),
    ]);

    assert_eq!(ids, vec![EntityId(1), EntityId(2), EntityId(3)]);
    assert_eq!(store.len(), 3);
    for id in ids {
        assert_eq!(store.snapshot(id).unwrap().type_name, "minecraft:cow");
    }
}

#[test]
fn damage_reduces_health_and_marks_killed_entities() {
    let mut store = EntityStore::new();
    let id = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));
    let initial = store.snapshot(id).unwrap();

    for invalid in [
        EntityDamageRequest {
            amount: 0.0,
            tick: 1,
            death_remove_tick: 21,
            villager_gossip_event: None,
        },
        EntityDamageRequest {
            amount: -1.0,
            tick: 1,
            death_remove_tick: 21,
            villager_gossip_event: None,
        },
        EntityDamageRequest {
            amount: f32::NAN,
            tick: 1,
            death_remove_tick: 21,
            villager_gossip_event: None,
        },
        EntityDamageRequest {
            amount: 1.0,
            tick: 21,
            death_remove_tick: 20,
            villager_gossip_event: None,
        },
    ] {
        assert!(store.damage(id, invalid).is_none());
        assert_eq!(store.snapshot(id), Some(initial.clone()));
    }

    let hit = store
        .damage(
            id,
            EntityDamageRequest {
                amount: 5.0,
                tick: 1,
                death_remove_tick: 21,
                villager_gossip_event: None,
            },
        )
        .unwrap();
    assert!(!hit.killed);
    assert_eq!(hit.snapshot.health, 15.0);
    assert_eq!(hit.snapshot.lifecycle, EntityLifecycle::Alive);
    assert_eq!(hit.snapshot.retained.last_damage_tick, Some(1));
    assert_eq!(hit.snapshot.retained.death_remove_tick, None);

    let lethal = store
        .damage(
            id,
            EntityDamageRequest {
                amount: 20.0,
                tick: 2,
                death_remove_tick: 22,
                villager_gossip_event: None,
            },
        )
        .unwrap();
    assert!(lethal.killed);
    assert_eq!(lethal.snapshot.health, 0.0);
    assert_eq!(lethal.snapshot.lifecycle, EntityLifecycle::Despawning);
    assert_eq!(lethal.snapshot.retained.last_damage_tick, Some(2));
    assert_eq!(lethal.snapshot.retained.death_remove_tick, Some(22));
}

#[test]
fn remove_keeps_remaining_entity_addressable() {
    let mut store = EntityStore::new();
    let a = store.spawn(cow(Vec3::new(1.0, 64.0, 1.0)));
    let b = store.spawn(cow(Vec3::new(2.0, 64.0, 2.0)));
    let c = store.spawn(cow(Vec3::new(3.0, 64.0, 3.0)));

    let removed = store.remove(b).unwrap();

    assert_eq!(removed.id, b);
    assert!(!store.contains(b));
    assert!(store.contains(a));
    assert!(store.contains(c));
    assert_eq!(store.len(), 2);
    assert_eq!(
        store.snapshot(c).unwrap().position,
        Vec3::new(3.0, 64.0, 3.0)
    );
}

#[test]
fn attributes_expose_vanilla_names_and_base_values() {
    let mut attrs = AttributeSet::vanilla_mob_defaults();
    attrs.set_base(AttributeKind::AttackDamage, 3.0);

    assert_eq!(
        AttributeKind::MaxHealth.vanilla_name(),
        "minecraft:max_health"
    );
    assert_eq!(attrs.base(&AttributeKind::MaxHealth), Some(20.0));
    assert_eq!(attrs.base(&AttributeKind::AttackDamage), Some(3.0));
    assert_eq!(AttributeKind::Scale.vanilla_name(), "minecraft:scale");
    assert_eq!(attrs.base(&AttributeKind::Scale), Some(1.0));
    assert_eq!(attrs.iter().count(), 5);
}

#[test]
fn position_ticks_can_be_split_into_ranges() {
    let mut store = EntityStore::new();
    let a = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    let b = store.spawn(cow(Vec3::new(10.0, 64.0, 10.0)));
    store.set_velocity(a, Vec3::new(1.0, 0.0, 0.0));
    store.set_velocity(b, Vec3::new(0.0, 0.0, 1.0));

    assert_eq!(store.batch_ranges(1), vec![0..1, 1..2]);
    store.tick_positions_in_range(0..1, 0.5);

    assert_eq!(
        store.snapshot(a).unwrap().position,
        Vec3::new(0.5, 64.0, 0.0)
    );
    assert_eq!(
        store.snapshot(b).unwrap().position,
        Vec3::new(10.0, 64.0, 10.0)
    );
}

#[test]
fn wander_goal_is_deterministic() {
    let mut a = EntityStore::new();
    let mut b = EntityStore::new();
    let entity_a = a.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    let entity_b = b.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    a.set_goal(
        entity_a,
        GoalState::Wander {
            speed: 0.2,
            period_ticks: 20,
        },
    );
    b.set_goal(
        entity_b,
        GoalState::Wander {
            speed: 0.2,
            period_ticks: 20,
        },
    );

    a.tick_goals(40);
    b.tick_goals(40);

    assert_eq!(
        a.snapshot(entity_a).unwrap().velocity,
        b.snapshot(entity_b).unwrap().velocity
    );
}

#[test]
fn authoritative_goal_batch_runs_one_ecs_input_stage() {
    let mut store = EntityStore::new();
    let first = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    let second = store.spawn(cow(Vec3::new(2.0, 64.0, 0.0)));
    let before = store.input_ai_stage_runs_for_test();

    let goals = [
        (
            first,
            GoalState::Wander {
                speed: 0.2,
                period_ticks: 20,
            },
        ),
        (
            second,
            GoalState::FollowPosition {
                target: Vec3::new(8.0, 64.0, 0.0),
                speed: 3.0,
            },
        ),
    ];

    assert_eq!(store.set_goals(goals.clone()), 2);

    assert_eq!(store.input_ai_stage_runs_for_test() - before, 1);
    assert!(matches!(
        store.snapshot(first).unwrap().goal,
        GoalState::Wander { .. }
    ));
    assert!(matches!(
        store.snapshot(second).unwrap().goal,
        GoalState::FollowPosition { .. }
    ));

    let after_change = store.input_ai_stage_runs_for_test();
    assert_eq!(store.set_goals(goals), 2);
    assert_eq!(store.input_ai_stage_runs_for_test(), after_change);
}

#[test]
fn follow_target_sets_horizontal_velocity() {
    let mut store = EntityStore::new();
    let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    let target = store.spawn(cow(Vec3::new(3.0, 64.0, 4.0)));
    store.set_goal(follower, GoalState::FollowTarget { target, speed: 0.5 });

    store.tick_goals(1);

    let velocity = store.snapshot(follower).unwrap().velocity;
    assert!((velocity.x - 0.3).abs() < 0.000_001);
    assert!((velocity.z - 0.4).abs() < 0.000_001);
}

#[test]
fn ground_goal_turns_body_and_head_without_snapping() {
    let mut store = EntityStore::new();
    let mut entity = cow(Vec3::new(0.0, 64.0, 0.0));
    entity.goal = GoalState::FollowPosition {
        target: Vec3::new(10.0, 64.0, 0.0),
        speed: 1.0,
    };
    let id = store.spawn(entity);

    store.tick_goals(1);
    let first = store.snapshot(id).unwrap().rotation;
    assert_eq!(first.yaw, -20.0);
    assert_eq!(first.head_yaw, -30.0);

    store.tick_goals(2);
    let second = store.snapshot(id).unwrap().rotation;
    assert_eq!(second.yaw, -40.0);
    assert_eq!(second.head_yaw, -60.0);
}

#[test]
fn goal_tick_stats_report_applied_and_skipped_ai_decisions() {
    let mut store = EntityStore::new();
    let idle = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    let follower = store.spawn(cow(Vec3::new(2.0, 64.0, 0.0)));
    let despawning = store.spawn(cow(Vec3::new(4.0, 64.0, 0.0)));
    store.set_velocity(idle, Vec3::new(1.0, 2.0, 3.0));
    store.set_goal(
        follower,
        GoalState::FollowTarget {
            target: EntityId(99_999),
            speed: 0.5,
        },
    );
    store.mark_despawning(despawning);

    let stats = store.tick_goals_with_stats(1);

    assert_eq!(
        stats,
        GoalTickStats {
            alive_entities: 2,
            decisions_applied: 2,
            skipped_non_alive: 1,
            missing_follow_targets: 1,
            pathing_moves: 0,
            pathing_blocked: 0,
            pathing_unloaded: 0,
        }
    );
    assert_eq!(
        store.snapshot(idle).unwrap().velocity,
        Vec3::new(0.0, 2.0, 0.0)
    );
    assert_eq!(store.snapshot(follower).unwrap().velocity, Vec3::ZERO);
    assert_eq!(
        store.snapshot(despawning).unwrap().lifecycle,
        EntityLifecycle::Despawning
    );
}

#[test]
fn follow_position_sets_horizontal_velocity() {
    let mut store = EntityStore::new();
    let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    store.set_velocity(follower, Vec3::new(0.0, -0.25, 0.0));
    store.set_goal(
        follower,
        GoalState::FollowPosition {
            target: Vec3::new(0.0, 65.0, 4.0),
            speed: 0.5,
        },
    );

    store.tick_goals(1);

    let velocity = store.snapshot(follower).unwrap().velocity;
    assert_eq!(velocity.x, 0.0);
    assert_eq!(velocity.y, -0.25);
    assert!((velocity.z - 0.5).abs() < 0.000_001);
}

#[test]
fn boat_vehicle_mount_steer_and_dismount_updates_runtime_store() {
    let mut store = EntityStore::new();
    let passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    let boat = store.spawn(SpawnEntity::vehicle(
        VehicleKind::Boat,
        15,
        "minecraft:oak_boat",
        Vec3::new(10.0, 63.0, 10.0),
    ));

    store.mount_vehicle(boat, passenger).unwrap();
    assert_eq!(store.vehicle_for_passenger(passenger), Some(boat));

    store
        .apply_vehicle_input(
            boat,
            passenger,
            VehicleInput {
                right: true,
                forward: true,
                ..VehicleInput::default()
            },
        )
        .unwrap();
    let steered = store.snapshot(boat).unwrap();
    assert_eq!(steered.vehicle.unwrap().passenger, Some(passenger));
    assert_eq!(steered.rotation.yaw, 4.0);
    assert_eq!(steered.rotation.head_yaw, 4.0);
    assert!(steered.velocity.horizontal_len() > 0.0);

    store.tick_positions(1.0);
    let moved = store.snapshot(boat).unwrap();
    assert_ne!(moved.position, Vec3::new(10.0, 63.0, 10.0));

    store.dismount_vehicle(boat, passenger).unwrap();
    assert_eq!(store.vehicle_for_passenger(passenger), None);
    assert_eq!(
        store.snapshot(boat).unwrap().vehicle.unwrap().passenger,
        None
    );
}

#[test]
fn authoritative_vehicle_mount_steer_and_dismount_updates_ecs() {
    let mut store = EntityStore::new();
    let passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    let boat = store.spawn(SpawnEntity::vehicle(
        VehicleKind::Boat,
        15,
        "minecraft:oak_boat",
        Vec3::new(10.0, 63.0, 10.0),
    ));

    store.mount_vehicle(boat, passenger).unwrap();
    store
        .apply_vehicle_input(boat, passenger, VehicleInput::forward())
        .unwrap();
    assert!(store.snapshot(boat).unwrap().velocity.horizontal_len() > 0.0);

    store.dismount_vehicle(boat, passenger).unwrap();
    assert_eq!(store.vehicle_for_passenger(passenger), None);
}

#[test]
fn authoritative_animal_breeding_state_round_trips_through_ecs_snapshot() {
    let mut store = EntityStore::new();
    let mut entity = cow(Vec3::new(2.0, 64.0, 3.0));
    entity.animal = Some(AnimalBreedingState {
        age_ticks: BABY_START_AGE_TICKS,
        love_ticks: 0,
        sheep_wool: None,
    });

    let id = store.spawn(entity);
    assert_eq!(
        store.snapshot(id).unwrap().animal,
        Some(AnimalBreedingState {
            age_ticks: BABY_START_AGE_TICKS,
            love_ticks: 0,
            sheep_wool: None,
        })
    );

    assert!(store.set_animal_state(
        id,
        AnimalBreedingState {
            age_ticks: 0,
            love_ticks: ANIMAL_LOVE_DURATION_TICKS,
            sheep_wool: None,
        },
    ));
    assert_eq!(
        store.snapshot(id).unwrap().animal,
        Some(AnimalBreedingState {
            age_ticks: 0,
            love_ticks: ANIMAL_LOVE_DURATION_TICKS,
            sheep_wool: None,
        })
    );
}

#[test]
fn breeding_tick_index_tracks_state_and_lifecycle_changes() {
    let mut store = EntityStore::new();
    let mut idle = cow(Vec3::new(1.0, 64.0, 1.0));
    idle.animal = Some(AnimalBreedingState::adult());
    let idle_id = store.spawn(idle);
    let mut baby = cow(Vec3::new(2.0, 64.0, 2.0));
    baby.animal = Some(AnimalBreedingState::baby());
    let baby_id = store.spawn(baby);
    let mut in_love = cow(Vec3::new(3.0, 64.0, 3.0));
    in_love.animal = Some(AnimalBreedingState {
        age_ticks: 0,
        love_ticks: ANIMAL_LOVE_DURATION_TICKS,
        sheep_wool: None,
    });
    let love_id = store.spawn(in_love);

    let mut seen = Vec::new();
    store.visit_breeding_tick_entities(|entity| seen.push(entity.id));
    assert_eq!(seen, vec![baby_id, love_id]);

    assert!(store.set_animal_state(
        idle_id,
        AnimalBreedingState {
            age_ticks: 0,
            love_ticks: ANIMAL_LOVE_DURATION_TICKS,
            sheep_wool: None,
        },
    ));
    assert!(store.set_animal_state(baby_id, AnimalBreedingState::adult()));
    assert!(store.mark_despawning(love_id));

    let mut seen = Vec::new();
    store.visit_breeding_tick_entities(|entity| seen.push(entity.id));
    assert_eq!(seen, vec![idle_id]);

    assert!(store.remove(idle_id).is_some());
    let mut seen = Vec::new();
    store.visit_breeding_tick_entities(|entity| seen.push(entity.id));
    assert!(seen.is_empty());
}

#[test]
fn sheep_index_intersects_candidates_and_tracks_state_changes() {
    let mut store = EntityStore::new();
    let mut near_sheep = cow(Vec3::new(1.0, 64.0, 1.0));
    near_sheep.type_id = 7;
    near_sheep.type_name = "minecraft:sheep".to_owned();
    near_sheep.animal = Some(AnimalBreedingState::adult_sheep(SheepColor::White));
    let near_sheep_id = store.spawn(near_sheep);
    let mut far_sheep = cow(Vec3::new(160.0, 64.0, 1.0));
    far_sheep.type_id = 7;
    far_sheep.type_name = "minecraft:sheep".to_owned();
    far_sheep.animal = Some(AnimalBreedingState::adult_sheep(SheepColor::Black));
    let far_sheep_id = store.spawn(far_sheep);
    let mut cow = cow(Vec3::new(2.0, 64.0, 1.0));
    cow.animal = Some(AnimalBreedingState::adult());
    let cow_id = store.spawn(cow);

    let mut seen = Vec::new();
    store.visit_sheep_entities_for_ids(&HashSet::from([near_sheep_id, cow_id]), |entity| {
        seen.push(entity.id)
    });
    assert_eq!(seen, vec![near_sheep_id]);
    assert!(!seen.contains(&far_sheep_id));

    assert!(store.set_animal_state(near_sheep_id, AnimalBreedingState::adult()));
    assert!(store.set_animal_state(cow_id, AnimalBreedingState::adult_sheep(SheepColor::White),));
    let mut seen = Vec::new();
    store.visit_sheep_entities_for_ids(&HashSet::from([near_sheep_id, cow_id]), |entity| {
        seen.push(entity.id)
    });
    assert!(seen.is_empty());

    assert!(store.set_animal_state(
        near_sheep_id,
        AnimalBreedingState::adult_sheep(SheepColor::White),
    ));
    assert!(store.mark_despawning(near_sheep_id));
    let mut seen = Vec::new();
    store.visit_sheep_entities_for_ids(&HashSet::from([near_sheep_id]), |entity| {
        seen.push(entity.id)
    });
    assert!(seen.is_empty());
}

#[test]
fn authoritative_sheep_wool_state_round_trips_through_ecs_snapshot() {
    let mut store = EntityStore::new();
    let mut entity = SpawnEntity::new(7, "minecraft:sheep", Vec3::new(2.0, 64.0, 3.0));
    entity.animal = Some(AnimalBreedingState::adult_sheep(SheepColor::White));

    let id = store.spawn(entity);
    let mut animal = store.snapshot(id).unwrap().animal.unwrap();
    assert_eq!(
        animal.sheep_wool,
        Some(SheepWoolState {
            color: SheepColor::White,
            sheared: false,
        })
    );

    animal.sheep_wool.as_mut().unwrap().sheared = true;
    assert!(store.set_animal_state(id, animal));
    assert_eq!(
        store
            .snapshot(id)
            .unwrap()
            .animal
            .unwrap()
            .sheep_wool
            .unwrap()
            .packed_metadata(),
        0x10
    );
}

#[test]
fn vehicle_releases_removed_passenger() {
    let mut store = EntityStore::new();
    let passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    let boat = store.spawn(SpawnEntity::vehicle(
        VehicleKind::Boat,
        15,
        "minecraft:oak_boat",
        Vec3::new(10.0, 63.0, 10.0),
    ));

    store.mount_vehicle(boat, passenger).unwrap();
    assert_eq!(store.vehicle_for_passenger(passenger), Some(boat));
    assert_eq!(store.remove(passenger).unwrap().id, passenger);
    assert_eq!(store.vehicle_for_passenger(passenger), None);
}

#[test]
fn vehicle_mount_rejects_non_vehicle_and_double_mount() {
    let mut store = EntityStore::new();
    let first_passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    let second_passenger = store.spawn(cow(Vec3::new(1.0, 64.0, 0.0)));
    let cow = store.spawn(cow(Vec3::new(2.0, 64.0, 0.0)));
    let boat = store.spawn(SpawnEntity::vehicle(
        VehicleKind::Boat,
        15,
        "minecraft:oak_boat",
        Vec3::new(3.0, 63.0, 0.0),
    ));

    assert_eq!(
        store.mount_vehicle(cow, first_passenger),
        Err(VehicleError::NotVehicle)
    );
    store.mount_vehicle(boat, first_passenger).unwrap();
    assert_eq!(
        store.mount_vehicle(boat, second_passenger),
        Err(VehicleError::AlreadyMounted)
    );
    assert_eq!(
        store.mount_vehicle(cow, first_passenger),
        Err(VehicleError::PassengerAlreadyMounted)
    );
}

#[test]
fn vehicle_mount_rejects_self_mount_and_cycles() {
    let mut store = EntityStore::new();
    let first_boat = store.spawn(SpawnEntity::vehicle(
        VehicleKind::Boat,
        15,
        "minecraft:oak_boat",
        Vec3::new(0.0, 63.0, 0.0),
    ));
    let second_boat = store.spawn(SpawnEntity::vehicle(
        VehicleKind::Boat,
        15,
        "minecraft:oak_boat",
        Vec3::new(3.0, 63.0, 0.0),
    ));

    assert_eq!(
        store.mount_vehicle(first_boat, first_boat),
        Err(VehicleError::SelfMount)
    );
    store.mount_vehicle(second_boat, first_boat).unwrap();
    assert_eq!(
        store.mount_vehicle(first_boat, second_boat),
        Err(VehicleError::Cycle)
    );
}

#[test]
fn vehicle_mount_and_input_reject_non_alive_entities() {
    let mut store = EntityStore::new();
    let passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    let despawning_passenger = store.spawn(cow(Vec3::new(1.0, 64.0, 0.0)));
    let boat = store.spawn(SpawnEntity::vehicle(
        VehicleKind::Boat,
        15,
        "minecraft:oak_boat",
        Vec3::new(3.0, 63.0, 0.0),
    ));
    let despawning_boat = store.spawn(SpawnEntity::vehicle(
        VehicleKind::Boat,
        15,
        "minecraft:oak_boat",
        Vec3::new(6.0, 63.0, 0.0),
    ));

    store.mark_despawning(despawning_passenger);
    store.mark_despawning(despawning_boat);

    assert_eq!(
        store.mount_vehicle(boat, despawning_passenger),
        Err(VehicleError::InvalidLifecycle)
    );
    assert_eq!(
        store.mount_vehicle(despawning_boat, passenger),
        Err(VehicleError::InvalidLifecycle)
    );

    store.mount_vehicle(boat, passenger).unwrap();
    store.mark_despawning(passenger);
    assert_eq!(
        store.apply_vehicle_input(boat, passenger, VehicleInput::forward()),
        Err(VehicleError::InvalidLifecycle)
    );
}

#[test]
fn insert_snapshot_keeps_valid_vehicle_graphs() {
    let mut store = EntityStore::new();
    let passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    let vehicle = EntityId(40);

    assert!(store.insert_snapshot(vehicle_snapshot(
        vehicle,
        EntityLifecycle::Alive,
        Some(passenger),
    )));

    assert_eq!(store.vehicle_for_passenger(passenger), Some(vehicle));
    assert_eq!(
        store.snapshot(vehicle).unwrap().vehicle.unwrap().passenger,
        Some(passenger)
    );
}

#[test]
fn insert_snapshot_keeps_authoritative_passenger_mount() {
    let mut store = EntityStore::new();
    let passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    let vehicle = EntityId(40);

    assert!(store.insert_snapshot(vehicle_snapshot(
        vehicle,
        EntityLifecycle::Alive,
        Some(passenger),
    )));

    assert_eq!(store.vehicle_for_passenger(passenger), Some(vehicle));
}

#[test]
fn insert_snapshot_drops_invalid_vehicle_graphs() {
    let mut store = EntityStore::new();
    let alive_passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    let despawning_passenger = store.spawn(cow(Vec3::new(1.0, 64.0, 0.0)));
    store.mark_despawning(despawning_passenger);

    let cases = [
        (EntityId(40), EntityLifecycle::Alive, Some(EntityId(40))),
        (EntityId(41), EntityLifecycle::Alive, Some(EntityId(999))),
        (
            EntityId(42),
            EntityLifecycle::Alive,
            Some(despawning_passenger),
        ),
        (
            EntityId(43),
            EntityLifecycle::Despawning,
            Some(alive_passenger),
        ),
    ];

    for (vehicle, lifecycle, passenger) in cases {
        assert!(store.insert_snapshot(vehicle_snapshot(vehicle, lifecycle, passenger)));
        assert_eq!(store.snapshot(vehicle).unwrap().vehicle, None);
    }
    assert_eq!(store.vehicle_for_passenger(alive_passenger), None);
    assert_eq!(store.vehicle_for_passenger(despawning_passenger), None);
}

#[test]
fn insert_snapshot_drops_vehicle_graph_cycles() {
    let mut store = EntityStore::new();
    let existing_vehicle = store.spawn(SpawnEntity::vehicle(
        VehicleKind::Boat,
        15,
        "minecraft:oak_boat",
        Vec3::new(3.0, 63.0, 0.0),
    ));
    let inserted_vehicle = EntityId(40);
    let mut existing_state = store.snapshot(existing_vehicle).unwrap().vehicle.unwrap();
    existing_state.passenger = Some(inserted_vehicle);
    assert!(store.set_vehicle_state(existing_vehicle, Some(existing_state)));

    assert!(store.insert_snapshot(vehicle_snapshot(
        inserted_vehicle,
        EntityLifecycle::Alive,
        Some(existing_vehicle),
    )));

    assert_eq!(store.snapshot(inserted_vehicle).unwrap().vehicle, None);
}

#[test]
fn removing_passenger_clears_vehicle_mount() {
    let mut store = EntityStore::new();
    let passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    let boat = store.spawn(SpawnEntity::vehicle(
        VehicleKind::Boat,
        15,
        "minecraft:oak_boat",
        Vec3::new(3.0, 63.0, 0.0),
    ));
    store.mount_vehicle(boat, passenger).unwrap();

    store.remove(passenger).unwrap();

    assert_eq!(
        store.snapshot(boat).unwrap().vehicle.unwrap().passenger,
        None
    );
    assert_eq!(store.vehicle_for_passenger(passenger), None);
}

#[test]
fn minecart_mount_exists_but_steering_is_explicitly_unsupported() {
    let mut store = EntityStore::new();
    let passenger = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    let minecart = store.spawn(SpawnEntity::vehicle(
        VehicleKind::Minecart,
        63,
        "minecraft:minecart",
        Vec3::new(3.0, 63.0, 0.0),
    ));
    store.mount_vehicle(minecart, passenger).unwrap();

    assert_eq!(
        store.apply_vehicle_input(minecart, passenger, VehicleInput::forward()),
        Err(VehicleError::UnsupportedSteering)
    );
}

#[test]
fn regional_goal_selection_keeps_shulker_bullet_retargeting() {
    let mut store = EntityStore::new();
    let position = Vec3::new(0.5, 64.0, 0.5);
    let projectile_position = projectile_26_1_2::Vec3::new(position.x, position.y, position.z);
    let bounds = projectile_26_1_2::Aabb::new(0.4, 64.0, 0.4, 0.6, 64.2, 0.6).unwrap();
    let mut bullet = SpawnEntity::new(113, "minecraft:shulker_bullet", position);
    bullet.retained.hurting_projectile_state = Some(
        projectile_26_1_2::HurtingProjectileState::new(
            None,
            projectile_position,
            bounds,
            projectile_26_1_2::Vec3::new(1.0, 0.0, 0.0),
            projectile_26_1_2::Rotation::new(0.0, 0.0),
            0.1,
        )
        .unwrap(),
    );
    bullet.retained.shulker_bullet = Some(EntityShulkerBulletState::new(42));
    let bullet = store.spawn(bullet);
    let inputs = regional::RegionalGoalTickInputs {
        combat_targets: std::sync::Arc::new(HashMap::from([(42, Vec3::new(10.5, 64.0, 0.5))])),
        ..regional::RegionalGoalTickInputs::default()
    };

    let selection = store.goal_tick_selection(
        regional::RegionKey::from_position(position).unwrap(),
        1,
        &HashSet::from([bullet]),
        &inputs,
    );

    let [(expected, next)] = selection.snapshot_overrides.as_slice() else {
        panic!("shulker bullet retarget must remain on the fallback goal route");
    };
    assert_eq!(expected.id, bullet);
    assert_eq!(next.velocity, Vec3::new(0.03, 0.0, 0.0));
}

#[test]
fn bounded_pathing_moves_over_flat_loaded_terrain() {
    let mut store = EntityStore::new();
    let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    store.set_velocity(follower, Vec3::new(0.0, -0.25, 0.0));
    store.set_goal(
        follower,
        GoalState::FollowPosition {
            target: Vec3::new(4.0, 64.0, 0.0),
            speed: 0.5,
        },
    );
    let probe = TestPathingProbe::new(PathingProbeResult::Walkable);

    let stats = store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);

    let velocity = store.snapshot(follower).unwrap().velocity;
    assert_eq!(stats.pathing_moves, 1);
    assert!((velocity.x - 0.5).abs() < 0.000_001);
    assert_eq!(velocity.y, -0.25);
    assert_eq!(velocity.z, 0.0);
}

#[test]
fn bounded_pathing_probes_speed_scaled_next_position() {
    let mut store = EntityStore::new();
    let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    store.set_goal(
        follower,
        GoalState::FollowPosition {
            target: Vec3::new(4.0, 64.0, 0.0),
            speed: 0.25,
        },
    );
    let probe = TestPathingProbe::new(PathingProbeResult::Unloaded).with(
        0.0125,
        64.0,
        0.0,
        PathingProbeResult::Walkable,
    );

    let stats = store.tick_goals_with_pathing(
        1,
        &probe,
        PathingBudget {
            max_candidates_per_entity: 1,
            ..PathingBudget::DEFAULT
        },
    );

    let velocity = store.snapshot(follower).unwrap().velocity;
    assert_eq!(stats.pathing_moves, 1);
    assert!((velocity.x - 0.25).abs() < 0.000_001);
    assert_eq!(velocity.y, 0.0);
    assert_eq!(velocity.z, 0.0);
}

#[test]
fn prepared_goal_tick_exposes_exact_probe_positions() {
    let mut store = EntityStore::new();
    let follower = store.spawn(cow(Vec3::new(15.9, 64.0, 0.5)));
    store.set_goal(
        follower,
        GoalState::FollowPosition {
            target: Vec3::new(20.0, 64.0, 0.5),
            speed: 4.0,
        },
    );
    let prepared = store.prepare_goal_tick_with_pathing_for_ids(1, &HashSet::from([follower]));
    let mut positions = Vec::new();

    prepared.visit_pathing_probe_positions(
        PathingBudget {
            max_candidates_per_entity: 1,
            ..PathingBudget::DEFAULT
        },
        |entity, position| positions.push((entity, position)),
    );

    assert_eq!(
        positions,
        vec![
            (follower, Vec3::new(15.9, 64.0, 0.5)),
            (follower, Vec3::new(16.1, 64.0, 0.5)),
            (follower, Vec3::new(16.1, 65.0, 0.5)),
            (follower, Vec3::new(16.9, 64.0, 0.5)),
            (follower, Vec3::new(16.9, 63.0, 0.5)),
            (follower, Vec3::new(15.9, 64.0, 1.5)),
            (follower, Vec3::new(15.9, 63.0, 1.5)),
            (follower, Vec3::new(15.9, 64.0, -0.5)),
            (follower, Vec3::new(15.9, 63.0, -0.5)),
            (follower, Vec3::new(14.9, 64.0, 0.5)),
            (follower, Vec3::new(14.9, 63.0, 0.5)),
        ]
    );
}

#[test]
fn prepared_goal_tick_declares_every_position_resolve_may_probe() {
    struct DeclaredOnlyProbe {
        declared: HashSet<(i64, i64, i64)>,
    }

    impl PathingProbe for DeclaredOnlyProbe {
        fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
            assert!(
                self.declared.contains(&pathing_test_key(position)),
                "resolve probed undeclared position {position:?}"
            );
            if position.y > 64.0 {
                PathingProbeResult::Walkable
            } else {
                PathingProbeResult::Blocked
            }
        }
    }

    let mut store = EntityStore::new();
    let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    store.set_goal(
        follower,
        GoalState::FollowPosition {
            target: Vec3::new(4.0, 64.0, 0.0),
            speed: 0.5,
        },
    );
    let prepared = store.prepare_goal_tick_with_pathing_for_ids(1, &HashSet::from([follower]));
    let mut declared = HashSet::new();
    prepared.visit_pathing_probe_positions(PathingBudget::DEFAULT, |_, position| {
        declared.insert(pathing_test_key(position));
    });

    let probe = DeclaredOnlyProbe { declared };
    let _ = prepared.resolve(&probe, PathingBudget::DEFAULT);
}

#[test]
fn prepared_goal_tick_declares_retained_node_and_fallback_positions() {
    struct DeclaredFallbackProbe {
        declared: HashSet<(i64, i64, i64)>,
        calls: Cell<usize>,
    }

    impl PathingProbe for DeclaredFallbackProbe {
        fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
            assert!(
                self.declared.contains(&pathing_test_key(position)),
                "fallback resolve probed undeclared position {position:?}"
            );
            let call = self.calls.get();
            self.calls.set(call + 1);
            if call < 2 {
                PathingProbeResult::Blocked
            } else {
                PathingProbeResult::Walkable
            }
        }
    }

    let id = EntityId(7);
    let target = Vec3::new(4.0, 64.0, 0.0);
    let retained = RetainedPathState {
        nodes: [Vec3::new(1.5, 64.0, 1.5), target],
        node_count: 2,
        target,
        target_revision: 1,
        has_target: true,
        ..RetainedPathState::default()
    };
    let prepared = PreparedGoalTick {
        tick: 2,
        active_ids: None,
        passive_decisions: 0,
        pathing_requests: vec![GoalPathingRequest {
            id,
            expected_position: Vec3::new(0.0, 64.0, 0.0),
            expected_rotation: Rotation::ZERO,
            expected_velocity: Vec3::ZERO,
            expected_on_ground: true,
            expected_goal: GoalState::FollowPosition { target, speed: 1.0 },
            expected_path: retained,
            target,
            target_epoch: None,
            speed: 1.0,
            aquatic: None,
        }],
    };
    let mut declared = HashSet::new();
    prepared.visit_pathing_probe_positions(PathingBudget::DEFAULT, |_, position| {
        declared.insert(pathing_test_key(position));
    });
    let probe = DeclaredFallbackProbe {
        declared,
        calls: Cell::new(0),
    };

    let _ = prepared.resolve(&probe, PathingBudget::DEFAULT);

    assert_eq!(
        probe.calls.get(),
        4,
        "retained flat, step, overlap, and fallback direct probes must run in order"
    );
}

#[test]
fn bounded_pathing_probes_the_next_tick_position() {
    struct RecordingProbe(std::sync::Mutex<Vec<Vec3>>);

    impl PathingProbe for RecordingProbe {
        fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
            self.0.lock().unwrap().push(position);
            PathingProbeResult::Walkable
        }
    }

    let mut store = EntityStore::new();
    let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    store.set_goal(
        follower,
        GoalState::FollowPosition {
            target: Vec3::new(4.0, 64.0, 0.0),
            speed: 2.0,
        },
    );
    let probe = RecordingProbe(std::sync::Mutex::new(Vec::new()));

    store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);

    let positions = probe.0.into_inner().unwrap();
    assert_eq!(positions.len(), 1);
    assert!((positions[0].x - 0.1).abs() < 1.0e-9);
    assert_eq!(positions[0].y, 64.0);
    assert_eq!(positions[0].z, 0.0);
}

#[test]
fn bounded_pathing_steps_up_one_block_when_flat_route_is_blocked() {
    let mut store = EntityStore::new();
    let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    store.set_goal(
        follower,
        GoalState::FollowPosition {
            target: Vec3::new(4.0, 64.0, 0.0),
            speed: 0.5,
        },
    );
    let probe = TestPathingProbe::new(PathingProbeResult::Blocked).with(
        0.025,
        65.0,
        0.0,
        PathingProbeResult::Walkable,
    );

    let stats = store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);

    let velocity = store.snapshot(follower).unwrap().velocity;
    assert_eq!(stats.pathing_moves, 1);
    assert!((velocity.x - 0.5).abs() < 0.000_001);
    assert!((velocity.y - 0.5).abs() < 0.000_001);
    assert_eq!(velocity.z, 0.0);
}

#[test]
fn bounded_pathing_detours_around_blocked_direct_step() {
    let mut store = EntityStore::new();
    let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    store.set_goal(
        follower,
        GoalState::FollowPosition {
            target: Vec3::new(4.0, 64.0, 0.0),
            speed: 0.5,
        },
    );
    let side = 0.025 / 2.0_f64.sqrt();
    let probe = TestPathingProbe::new(PathingProbeResult::Blocked).with(
        side,
        64.0,
        side,
        PathingProbeResult::Walkable,
    );

    let stats = store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);

    let velocity = store.snapshot(follower).unwrap().velocity;
    assert_eq!(stats.pathing_moves, 1);
    assert!(velocity.x > 0.0);
    assert!(velocity.z.abs() > 0.0);
    assert!(velocity.horizontal_len() <= 0.5 + 0.000_001);
}

/// A wander target buried in tree leaves is unreachable: every walkable
/// detour step is farther away. Such a step must be refused instead of
/// walking the entity away from its target, which reads as spinning in
/// place when the entity then turns back.
#[test]
fn bounded_pathing_refuses_a_detour_that_moves_away_from_an_unreachable_target() {
    struct ThinWallAheadProbe;

    impl PathingProbe for ThinWallAheadProbe {
        fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
            if (0.02..0.1).contains(&position.z) && position.x.abs() < 0.5 {
                PathingProbeResult::Blocked
            } else {
                PathingProbeResult::Walkable
            }
        }
    }

    let mut store = EntityStore::new();
    let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    store.set_goal(
        follower,
        GoalState::FollowPosition {
            target: Vec3::new(0.0, 64.0, 4.0),
            speed: 1.0,
        },
    );

    let stats = store.tick_goals_with_pathing(1, &ThinWallAheadProbe, PathingBudget::DEFAULT);

    assert_eq!(stats.pathing_blocked, 1);
    assert_eq!(store.snapshot(follower).unwrap().velocity, Vec3::ZERO);
}

struct TwoBlockWallPathingProbe;

impl PathingProbe for TwoBlockWallPathingProbe {
    fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
        if (0.15..=2.2).contains(&position.x) && position.z.abs() < 0.1 {
            PathingProbeResult::Blocked
        } else {
            PathingProbeResult::Walkable
        }
    }
}

#[test]
fn retained_path_routes_around_two_block_wall_and_rejoins_target() {
    let mut store = EntityStore::new();
    let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    let target = Vec3::new(4.0, 64.0, 0.0);
    store.set_goal(follower, GoalState::FollowPosition { target, speed: 4.0 });
    let mut retained_direction = None;

    for tick in 1..=64 {
        store.tick_goals_with_pathing(tick, &TwoBlockWallPathingProbe, PathingBudget::DEFAULT);
        let snapshot = store.snapshot(follower).expect("follower snapshot");
        let direction = snapshot.velocity.horizontal_normalized();
        if tick == 1 {
            assert!(direction.z.abs() > 0.0, "wall must force a detour");
            retained_direction = Some(direction);
        } else if tick <= 4 {
            let retained = retained_direction.expect("initial detour direction");
            assert!(
                direction.x * retained.x + direction.z * retained.z > 0.999,
                "retained route must not oscillate while clearing the wall"
            );
        }
        store.tick_positions(0.05);
        let position = store.snapshot(follower).expect("moved follower").position;
        if (target.x - position.x).hypot(target.z - position.z) <= 0.25 {
            assert!(position.z.abs() <= 0.25, "route must rejoin the target");
            return;
        }
    }

    panic!("retained route did not reach the target within its bounded path budget");
}

#[test]
fn wander_retains_its_absolute_target_until_reached() {
    let mut store = EntityStore::new();
    let mut entity = cow(Vec3::new(0.0, 64.0, 0.0));
    entity.goal = GoalState::Wander {
        speed: 4.0,
        period_ticks: 40,
    };
    let id = store.spawn(entity);
    let probe = TestPathingProbe::new(PathingProbeResult::Walkable);

    store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);
    store.tick_positions(0.05);
    let prepared = store.prepare_goal_tick_with_pathing_for_ids(400, &HashSet::from([id]));
    let request = &prepared.pathing_requests[0];

    assert_eq!(request.expected_path.target_revision, 1);
    assert_eq!(
        request.target, request.expected_path.target,
        "elapsed time must not replace an unfinished Wander target"
    );
}

#[test]
fn wander_pauses_after_reaching_its_retained_target() {
    let mut store = EntityStore::new();
    let mut entity = cow(Vec3::new(0.0, 64.0, 0.0));
    entity.goal = GoalState::Wander {
        speed: 3.0,
        period_ticks: 40,
    };
    let id = store.spawn(entity);
    let probe = TestPathingProbe::new(PathingProbeResult::Walkable);

    let mut reached = None;
    let speed = 3.0;
    let max_ticks =
        (((WANDER_MIN_DISTANCE + WANDER_DISTANCE_SPREAD) / speed) * 20.0).ceil() as u64 + 80;
    for tick in 1..=max_ticks {
        store.tick_goals_with_pathing(tick, &probe, PathingBudget::DEFAULT);
        let prepared = store.prepare_goal_tick_with_pathing_for_ids(tick + 1, &HashSet::from([id]));
        let path = prepared.pathing_requests[0].expected_path;
        if path.target_reached {
            reached = Some((
                prepared.pathing_requests[0].target,
                path.resume_tick,
                store.snapshot(id).expect("wanderer snapshot").position,
            ));
            break;
        }
        store.tick_positions(PathingBudget::TICK_SECONDS);
    }
    let (target, resume_tick, position) = reached.expect("wanderer reaches its retained target");

    assert_eq!(store.snapshot(id).unwrap().velocity, Vec3::ZERO);
    let rotation = store.snapshot(id).unwrap().rotation;
    let paused =
        store.prepare_goal_tick_with_pathing_for_ids(resume_tick - 1, &HashSet::from([id]));
    assert_eq!(paused.pathing_requests[0].target, target);
    store.apply_prepared_goal_tick(paused.resolve(&probe, PathingBudget::DEFAULT));
    store.tick_positions(PathingBudget::TICK_SECONDS);
    assert_eq!(store.snapshot(id).unwrap().position, position);
    assert_eq!(store.snapshot(id).unwrap().rotation, rotation);

    let resumed = store.prepare_goal_tick_with_pathing_for_ids(resume_tick, &HashSet::from([id]));
    assert_ne!(resumed.pathing_requests[0].target, target);
}

#[test]
fn wander_targets_are_multiblock_and_not_synchronized() {
    let mut store = EntityStore::new();
    let position = Vec3::new(0.0, 64.0, 0.0);
    let mut first = cow(position);
    first.goal = GoalState::Wander {
        speed: 4.0,
        period_ticks: 40,
    };
    let second = first.clone();
    let first_id = store.spawn(first);
    let second_id = store.spawn(second);

    let prepared =
        store.prepare_goal_tick_with_pathing_for_ids(1, &HashSet::from([first_id, second_id]));
    let first_target = prepared.pathing_requests[0].target;
    let second_target = prepared.pathing_requests[1].target;
    for target in [first_target, second_target] {
        let distance = (target.x - position.x).hypot(target.z - position.z);
        assert!(
            (WANDER_MIN_DISTANCE..=WANDER_MIN_DISTANCE + WANDER_DISTANCE_SPREAD)
                .contains(&distance)
        );
    }
    assert_ne!(first_target, second_target);

    // A vanilla-sized stroll reads as a static world, so the rolled reach
    // must stay long-range. This samples the real roll path rather than
    // re-stating the constants.
    let mut shortest = f64::INFINITY;
    let mut longest: f64 = 0.0;
    for raw in 1..=64i32 {
        let (target, _) =
            wander_pathing_target(EntityId(raw), position, RetainedPathState::default(), 1, 40);
        let distance = (target.x - position.x).hypot(target.z - position.z);
        shortest = shortest.min(distance);
        longest = longest.max(distance);
    }
    assert!(shortest >= WANDER_MIN_DISTANCE);
    assert!(
        longest >= 24.0,
        "wander reach must stay long-range so the world visibly moves"
    );
}

#[test]
fn path_probe_budget_is_global_across_retained_fallback() {
    struct CountingBlockedProbe(Cell<usize>);

    impl PathingProbe for CountingBlockedProbe {
        fn can_stand_at(&self, _position: Vec3) -> PathingProbeResult {
            self.0.set(self.0.get() + 1);
            PathingProbeResult::Blocked
        }
    }

    let id = EntityId(7);
    let target = Vec3::new(4.0, 64.0, 0.0);
    let retained = RetainedPathState {
        nodes: [Vec3::new(1.5, 64.0, 1.5), target],
        node_count: 2,
        target,
        target_revision: 1,
        has_target: true,
        ..RetainedPathState::default()
    };
    let prepared = PreparedGoalTick {
        tick: 2,
        active_ids: None,
        passive_decisions: 0,
        pathing_requests: vec![GoalPathingRequest {
            id,
            expected_position: Vec3::new(0.0, 64.0, 0.0),
            expected_rotation: Rotation::ZERO,
            expected_velocity: Vec3::ZERO,
            expected_on_ground: true,
            expected_goal: GoalState::FollowPosition { target, speed: 1.0 },
            expected_path: retained,
            target,
            target_epoch: None,
            speed: 1.0,
            aquatic: None,
        }],
    };
    let probe = CountingBlockedProbe(Cell::new(0));
    let budget = PathingBudget {
        max_candidates_per_entity: 3,
        ..PathingBudget::DEFAULT
    };

    let _ = prepared.resolve(&probe, budget);

    assert_eq!(
        probe.0.get(),
        budget.max_candidates_per_entity,
        "the configured per-entity bound must be the actual probe-call ceiling"
    );
}

#[test]
fn retreat_candidate_is_reached_within_the_actual_probe_budget() {
    struct RetreatProbe(RefCell<Vec<Vec3>>);

    impl PathingProbe for RetreatProbe {
        fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
            self.0.borrow_mut().push(position);
            if position.x < 0.0 && position.y == 64.0 {
                PathingProbeResult::Walkable
            } else {
                PathingProbeResult::Blocked
            }
        }
    }

    let mut store = EntityStore::new();
    let mut entity = cow(Vec3::new(0.0, 64.0, 0.0));
    entity.goal = GoalState::FollowPosition {
        target: Vec3::new(4.0, 64.0, 0.0),
        speed: 1.0,
    };
    let id = store.spawn(entity);
    let probe = RetreatProbe(RefCell::new(Vec::new()));

    let stats = store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);

    assert_eq!(stats.pathing_moves, 1);
    assert!(store.snapshot(id).unwrap().velocity.x < 0.0);
    let visited = probe.0.borrow();
    assert!(
        visited[7].x < 0.0,
        "the eighth probe must be the retreat after the overlap check"
    );
    assert_eq!(
        visited.len(),
        PathingBudget::DEFAULT.max_candidates_per_entity
    );
}

struct MutableRetainedNodeProbe {
    initial_wall: Cell<bool>,
    blocked_next: Cell<Option<Vec3>>,
    visited: RefCell<Vec<Vec3>>,
}

impl PathingProbe for MutableRetainedNodeProbe {
    fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
        self.visited.borrow_mut().push(position);
        if self.initial_wall.get()
            && (position.x - 0.2).abs() < 0.000_001
            && position.z.abs() < 0.000_001
        {
            return PathingProbeResult::Blocked;
        }
        if self.blocked_next.get().is_some_and(|blocked| {
            (position.x - blocked.x).abs() < 0.000_001 && (position.z - blocked.z).abs() < 0.000_001
        }) {
            return PathingProbeResult::Blocked;
        }
        PathingProbeResult::Walkable
    }
}

#[test]
fn retained_path_recomputes_when_next_node_becomes_blocked() {
    let mut store = EntityStore::new();
    let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    let target = Vec3::new(4.0, 64.0, 0.0);
    store.set_goal(follower, GoalState::FollowPosition { target, speed: 4.0 });
    let probe = MutableRetainedNodeProbe {
        initial_wall: Cell::new(true),
        blocked_next: Cell::new(None),
        visited: RefCell::new(Vec::new()),
    };

    store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);
    let first_velocity = store.snapshot(follower).expect("first decision").velocity;
    assert!(first_velocity.z.abs() > 0.0, "wall must force a detour");
    store.tick_positions(0.05);

    let position = store.snapshot(follower).expect("moved follower").position;
    let retained_next = Vec3::new(
        position.x + first_velocity.x * 0.05,
        position.y,
        position.z + first_velocity.z * 0.05,
    );
    probe.initial_wall.set(false);
    probe.blocked_next.set(Some(retained_next));
    probe.visited.borrow_mut().clear();

    store.tick_goals_with_pathing(2, &probe, PathingBudget::DEFAULT);

    let visited = probe.visited.borrow();
    assert_eq!(
        visited.first().copied(),
        Some(retained_next),
        "the retained next node must be validated before recomputation"
    );
    let velocity = store
        .snapshot(follower)
        .expect("recomputed decision")
        .velocity;
    assert!(velocity.x > 0.0);
    assert!(velocity.z.abs() < first_velocity.z.abs());
}

#[test]
fn retained_path_stops_after_bounded_no_progress() {
    let mut store = EntityStore::new();
    let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    store.set_goal(
        follower,
        GoalState::FollowPosition {
            target: Vec3::new(4.0, 64.0, 0.0),
            speed: 1.0,
        },
    );
    let probe = TestPathingProbe::new(PathingProbeResult::Walkable);
    let mut stopped = false;

    for tick in 1..=64 {
        let stats = store.tick_goals_with_pathing(tick, &probe, PathingBudget::DEFAULT);
        if stats.pathing_blocked == 1 {
            stopped = true;
            break;
        }
    }

    assert!(stopped, "no-progress recovery must have a finite bound");
    assert_eq!(store.snapshot(follower).unwrap().velocity, Vec3::ZERO);
}

#[test]
fn exhausted_wander_path_retargets_instead_of_retrying_forever() {
    let mut store = EntityStore::new();
    let mut entity = cow(Vec3::new(0.0, 64.0, 0.0));
    entity.goal = GoalState::Wander {
        speed: 1.0,
        period_ticks: 40,
    };
    let id = store.spawn(entity);
    let probe = TestPathingProbe::new(PathingProbeResult::Walkable);
    let mut exhausted_tick = None;

    for tick in 1..=64 {
        let target = store
            .prepare_goal_tick_with_pathing_for_ids(tick, &HashSet::from([id]))
            .pathing_requests[0]
            .target;
        let stats = store.tick_goals_with_pathing(tick, &probe, PathingBudget::DEFAULT);
        if stats.pathing_blocked == 1 {
            exhausted_tick = Some((tick, target));
            break;
        }
    }
    let (tick, exhausted_target) = exhausted_tick.expect("wander path exhausts");
    let next = store.prepare_goal_tick_with_pathing_for_ids(tick + 1, &HashSet::from([id]));

    assert_ne!(next.pathing_requests[0].target, exhausted_target);
}

#[test]
fn retained_follow_position_recovers_after_temporary_obstacle_clears() {
    struct MutableProbe(Cell<PathingProbeResult>);

    impl PathingProbe for MutableProbe {
        fn can_stand_at(&self, _position: Vec3) -> PathingProbeResult {
            self.0.get()
        }
    }

    let mut store = EntityStore::new();
    let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    store.set_goal(
        follower,
        GoalState::FollowPosition {
            target: Vec3::new(4.0, 64.0, 0.0),
            speed: 1.0,
        },
    );
    let probe = MutableProbe(Cell::new(PathingProbeResult::Blocked));

    for tick in 1..=RETAINED_PATH_RECOMPUTE_LIMIT.into() {
        store.tick_goals_with_pathing(tick, &probe, PathingBudget::DEFAULT);
    }
    assert_eq!(store.snapshot(follower).unwrap().velocity, Vec3::ZERO);

    probe.0.set(PathingProbeResult::Walkable);
    let stats = store.tick_goals_with_pathing(5, &probe, PathingBudget::DEFAULT);

    assert_eq!(stats.pathing_moves, 1);
    assert!(store.snapshot(follower).unwrap().velocity.x > 0.0);
}

#[test]
fn teleport_and_goal_change_invalidate_retained_path() {
    let mut store = EntityStore::new();
    let mut entity = cow(Vec3::new(0.0, 64.0, 0.0));
    entity.goal = GoalState::FollowPosition {
        target: Vec3::new(4.0, 64.0, 0.0),
        speed: 1.0,
    };
    let id = store.spawn(entity);
    store.tick_goals_with_pathing(
        1,
        &TestPathingProbe::new(PathingProbeResult::Walkable),
        PathingBudget::DEFAULT,
    );

    assert!(store.set_position(id, Vec3::new(2.0, 64.0, 2.0)));
    let after_teleport = store.prepare_goal_tick_with_pathing_for_ids(2, &HashSet::from([id]));
    assert_eq!(
        after_teleport.pathing_requests[0].expected_path,
        RetainedPathState::default()
    );

    store.tick_goals_with_pathing(
        2,
        &TestPathingProbe::new(PathingProbeResult::Walkable),
        PathingBudget::DEFAULT,
    );
    assert!(store.set_goal(
        id,
        GoalState::FollowPosition {
            target: Vec3::new(4.0, 64.0, 0.0),
            speed: 2.0,
        },
    ));
    let after_goal_change = store.prepare_goal_tick_with_pathing_for_ids(3, &HashSet::from([id]));
    assert_eq!(
        after_goal_change.pathing_requests[0].expected_path,
        RetainedPathState::default()
    );
}

#[test]
fn dense_ecs_pathing_requests_are_stably_ordered() {
    let mut store = EntityStore::new();
    let spawn_follower = |store: &mut EntityStore| {
        let mut entity = cow(Vec3::new(0.0, 64.0, 0.0));
        entity.goal = GoalState::FollowPosition {
            target: Vec3::new(4.0, 64.0, 0.0),
            speed: 1.0,
        };
        store.spawn(entity)
    };
    let first = spawn_follower(&mut store);
    let _second = spawn_follower(&mut store);
    let _third = spawn_follower(&mut store);
    store.remove(first).expect("first entity exists");
    let _fourth = spawn_follower(&mut store);

    let prepared = store.prepare_goal_tick_with_pathing_for_ids(
        1,
        &store.snapshots().map(|snapshot| snapshot.id).collect(),
    );
    let ids = prepared
        .pathing_requests
        .iter()
        .map(|request| request.id)
        .collect::<Vec<_>>();
    let mut sorted = ids.clone();
    sorted.sort_unstable();

    assert_eq!(ids, sorted);
}

#[test]
fn bounded_pathing_detours_around_unloaded_direct_step() {
    let mut store = EntityStore::new();
    let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    store.set_goal(
        follower,
        GoalState::FollowPosition {
            target: Vec3::new(4.0, 64.0, 0.0),
            speed: 0.5,
        },
    );
    let probe = TestPathingProbe::new(PathingProbeResult::Walkable).with(
        0.025,
        64.0,
        0.0,
        PathingProbeResult::Unloaded,
    );

    let stats = store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);

    let velocity = store.snapshot(follower).unwrap().velocity;
    assert_eq!(stats.pathing_moves, 1);
    assert!(velocity.x > 0.0);
    assert!(velocity.z.abs() > 0.0);
    assert!(velocity.horizontal_len() <= 0.5 + 0.000_001);
}

#[test]
fn bounded_pathing_refuses_blocked_terrain() {
    let mut store = EntityStore::new();
    let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    store.set_goal(
        follower,
        GoalState::FollowPosition {
            target: Vec3::new(4.0, 64.0, 0.0),
            speed: 0.5,
        },
    );
    let probe = TestPathingProbe::new(PathingProbeResult::Blocked);

    let stats = store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);

    assert_eq!(stats.pathing_blocked, 1);
    assert_eq!(store.snapshot(follower).unwrap().velocity, Vec3::ZERO);
}

#[test]
fn bounded_pathing_refuses_unloaded_terrain() {
    let mut store = EntityStore::new();
    let follower = store.spawn(cow(Vec3::new(0.0, 64.0, 0.0)));
    store.set_goal(
        follower,
        GoalState::FollowPosition {
            target: Vec3::new(4.0, 64.0, 0.0),
            speed: 0.5,
        },
    );
    let probe = TestPathingProbe::new(PathingProbeResult::Unloaded);

    let stats = store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);

    assert_eq!(stats.pathing_unloaded, 1);
    assert_eq!(store.snapshot(follower).unwrap().velocity, Vec3::ZERO);
}

#[test]
fn wander_uses_bounded_pathing_instead_of_walking_into_blocked_terrain() {
    let mut store = EntityStore::new();
    let mut entity = cow(Vec3::new(0.0, 64.0, 0.0));
    entity.goal = GoalState::Wander {
        speed: 0.2,
        period_ticks: 20,
    };
    let id = store.spawn(entity);
    let probe = TestPathingProbe::new(PathingProbeResult::Blocked);

    let stats = store.tick_goals_with_pathing(1, &probe, PathingBudget::DEFAULT);

    assert_eq!(stats.pathing_blocked, 1);
    assert_eq!(store.snapshot(id).unwrap().velocity, Vec3::ZERO);
}

#[test]
#[ignore = "explicit debug active-subset ECS benchmark"]
fn active_subset_ecs_density_benchmark_report() {
    const ENTITIES: usize = 10_000;
    const ACTIVE: usize = 32;
    const TICKS: u64 = 200;

    let mut store = EntityStore::new();
    let mut active_ids = HashSet::with_capacity(ACTIVE);
    for index in 0..ENTITIES {
        let mut entity = cow(Vec3::new(index as f64, 64.0, (index % 32) as f64));
        entity.goal = GoalState::Wander {
            speed: 0.2,
            period_ticks: 20,
        };
        let id = store.spawn(entity);
        if index < ACTIVE {
            active_ids.insert(id);
        }
    }
    let probe = TestPathingProbe::new(PathingProbeResult::Walkable);

    let started = std::time::Instant::now();
    for tick in 1..=TICKS {
        let prepared = store.prepare_goal_tick_with_pathing_for_ids(tick, &active_ids);
        store.apply_prepared_goal_tick(prepared.resolve(&probe, PathingBudget::DEFAULT));
        std::hint::black_box(store.alive_kinematics_for_ids(&active_ids));
    }
    let elapsed = started.elapsed();

    println!(
        "ENTITY_ACTIVE_SUBSET_BENCH entities={ENTITIES} active={ACTIVE} ticks={TICKS} total_us={} us_per_tick={}",
        elapsed.as_micros(),
        elapsed.as_micros() / u128::from(TICKS),
    );
}

fn runtime_density_store(entities: usize) -> EntityStore {
    let mut store = EntityStore::new();
    for index in 0..entities {
        let mut entity = cow(Vec3::new(index as f64, 64.0, (index % 32) as f64));
        entity.goal = GoalState::Wander {
            speed: 0.2,
            period_ticks: 20,
        };
        store.spawn(entity);
    }
    store
}

#[test]
#[ignore = "explicit debug dense active-set ECS benchmark"]
fn dense_active_set_ecs_benchmark_report() {
    const ENTITIES: usize = 1_000;
    const TICKS: u64 = 200;

    let mut full_query = runtime_density_store(ENTITIES);
    let mut indexed = runtime_density_store(ENTITIES);
    let active_ids = indexed
        .snapshots()
        .map(|snapshot| snapshot.id)
        .collect::<HashSet<_>>();
    let probe = TestPathingProbe::new(PathingProbeResult::Walkable);

    let full_started = std::time::Instant::now();
    for tick in 1..=TICKS {
        let prepared = full_query.prepare_goal_tick(tick, None, &HashMap::new());
        full_query.apply_prepared_goal_tick(prepared.resolve(&probe, PathingBudget::DEFAULT));
    }
    let full_elapsed = full_started.elapsed();

    let indexed_started = std::time::Instant::now();
    for tick in 1..=TICKS {
        let prepared = indexed.prepare_goal_tick_with_pathing_for_ids(tick, &active_ids);
        indexed.apply_prepared_goal_tick(prepared.resolve(&probe, PathingBudget::DEFAULT));
    }
    let indexed_elapsed = indexed_started.elapsed();

    assert_eq!(
        full_query.snapshots().collect::<Vec<_>>(),
        indexed.snapshots().collect::<Vec<_>>()
    );
    println!(
        "ENTITY_DENSE_ACTIVE_BENCH entities={ENTITIES} ticks={TICKS} full_us_per_tick={} indexed_us_per_tick={}",
        full_elapsed.as_micros() / u128::from(TICKS),
        indexed_elapsed.as_micros() / u128::from(TICKS),
    );
}
