use super::*;

struct OpenGround;
impl PathingProbe for OpenGround {
    fn can_stand_at(&self, _position: Vec3) -> PathingProbeResult {
        PathingProbeResult::Walkable
    }
}

#[test]
fn hurt_villager_panics_at_double_walk_speed_then_returns_to_walk_speed() {
    // Villager idle stroll is the full 0.5 movement-speed attribute in
    // blocks/second; the 26.1.2 villager `PanicGoal(this, 2.0)` doubles it.
    let mut store = EntityStore::new();
    let mut villager = SpawnEntity::new(11, "minecraft:villager", Vec3::new(0.5, 64.0, 0.5));
    villager.goal = GoalState::Wander {
        speed: 5.0,
        period_ticks: 100,
    };
    villager.velocity = Vec3::new(9.0, 0.0, 0.0);
    villager.retained.last_damage_tick = Some(10);
    let id = store.spawn(villager);
    store.tick_goals_with_pathing(15, &OpenGround, PathingBudget::DEFAULT);
    store.tick_positions(0.05);
    let fleeing = store.snapshot(id).unwrap();
    assert!(fleeing.position.x > 0.5);
    assert!((fleeing.velocity.x - 10.0).abs() < 1.0e-6);
    assert!(fleeing.velocity.z.abs() < 1.0e-6);

    store.tick_goals_with_pathing(110, &OpenGround, PathingBudget::DEFAULT);
    let walking = store.snapshot(id).unwrap();
    assert!((walking.velocity.x.hypot(walking.velocity.z) - 5.0).abs() < 1.0e-6);
}

#[test]
fn hurt_villager_walk_to_a_job_site_panics_instead_of_continuing_to_the_poi() {
    // Scheduled villager goals are `FollowPosition`; the panic override must
    // also engage there, fleeing along the knockback direction.
    let mut store = EntityStore::new();
    let mut villager = SpawnEntity::new(11, "minecraft:villager", Vec3::new(50.5, 64.0, 50.5));
    villager.goal = GoalState::FollowPosition {
        target: Vec3::new(58.5, 64.0, 50.5),
        speed: 3.0,
    };
    villager.velocity = Vec3::new(0.0, 0.0, -9.0);
    villager.retained.last_damage_tick = Some(10);
    let id = store.spawn(villager);
    store.tick_goals_with_pathing(15, &OpenGround, PathingBudget::DEFAULT);
    store.tick_positions(0.05);
    let fleeing = store.snapshot(id).unwrap();
    assert!(fleeing.position.z < 50.5, "{:?}", fleeing.position);
    assert!((fleeing.velocity.z + 6.0).abs() < 1.0e-6);
    assert!(fleeing.velocity.x.abs() < 1.0e-6);
}

#[test]
fn stationary_mob_with_a_target_re_aims_yaw_within_one_goal_tick() {
    let mut store = EntityStore::new();
    let target = SpawnEntity::new(11, "minecraft:cow", Vec3::new(10.5, 64.0, 0.5));
    let target_id = store.spawn(target);
    let mut hostile = SpawnEntity::new(12, "minecraft:spider", Vec3::new(0.5, 64.0, 0.5));
    hostile.goal = GoalState::FollowTarget {
        target: target_id,
        speed: 0.0,
    };
    hostile.rotation = Rotation {
        yaw: 0.0,
        pitch: 0.0,
        head_yaw: 0.0,
    };
    let hostile_id = store.spawn(hostile);
    store.tick_goals_with_pathing(5, &OpenGround, PathingBudget::DEFAULT);
    let aimed = store.snapshot(hostile_id).unwrap();
    // The target is due +x; vanilla yaw faces +x at -90 degrees.
    assert!((aimed.rotation.yaw + 90.0).abs() < 1.0e-4, "{aimed:?}");
    assert!((aimed.rotation.head_yaw + 90.0).abs() < 1.0e-4, "{aimed:?}");
}

#[test]
fn profession_registry_ids_match_the_26_1_2_report() {
    let cases = [
        (VillagerProfession::None, 0),
        (VillagerProfession::Armorer, 1),
        (VillagerProfession::Butcher, 2),
        (VillagerProfession::Cartographer, 3),
        (VillagerProfession::Cleric, 4),
        (VillagerProfession::Farmer, 5),
        (VillagerProfession::Fisherman, 6),
        (VillagerProfession::Fletcher, 7),
        (VillagerProfession::Leatherworker, 8),
        (VillagerProfession::Librarian, 9),
        (VillagerProfession::Mason, 10),
        (VillagerProfession::Nitwit, 11),
        (VillagerProfession::Shepherd, 12),
        (VillagerProfession::Toolsmith, 13),
        (VillagerProfession::Weaponsmith, 14),
    ];
    for (profession, id) in cases {
        assert_eq!(profession.profession_id(), id, "{profession:?}");
        assert_eq!(
            VillagerProfession::from_name(profession.name()),
            Some(profession)
        );
    }
    assert_eq!(VillagerProfession::from_name("minecraft:farmer"), None);
}
