use super::*;

struct OpenGround;
impl PathingProbe for OpenGround {
    fn can_stand_at(&self, _position: Vec3) -> PathingProbeResult {
        PathingProbeResult::Walkable
    }
}

#[test]
fn injured_animal_runs_along_knockback_then_returns_to_walking_speed() {
    let mut store = EntityStore::new();
    let mut animal = SpawnEntity::new(11, "minecraft:cow", Vec3::new(0.5, 64.0, 0.5));
    animal.animal = Some(AnimalBreedingState::adult());
    animal.goal = GoalState::Wander {
        speed: 2.0,
        period_ticks: 100,
    };
    animal.velocity = Vec3::new(9.0, 0.0, 0.0);
    animal.retained.last_damage_tick = Some(10);
    let id = store.spawn(animal);
    store.tick_goals_with_pathing(15, &OpenGround, PathingBudget::DEFAULT);
    store.tick_positions(0.05);
    let fleeing = store.snapshot(id).unwrap();
    assert!(fleeing.position.x > 0.5);
    assert!((fleeing.velocity.x - 4.0).abs() < 1.0e-6);
    assert!(fleeing.velocity.z.abs() < 1.0e-6);

    store.tick_goals_with_pathing(110, &OpenGround, PathingBudget::DEFAULT);
    let walking = store.snapshot(id).unwrap();
    assert!((walking.velocity.x.hypot(walking.velocity.z) - 2.0).abs() < 1.0e-6);
}
