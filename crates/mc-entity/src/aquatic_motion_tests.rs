use super::*;
use crate::{EntityPhysicsResult, EntityStage, EntityStore, GoalState, SpawnEntity};
use mc_physics::{Aabb, BlockMaterial, BlockSampler, EntityBody, PhysicsConfig};

struct Tank;
const BOUNDS: Aabb = Aabb {
    half_width: 0.25,
    height: 0.3,
};

impl BlockSampler for Tank {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        if !(0..8).contains(&x) || !(0..8).contains(&z) || y < 0 || (x == 4 && z < 5) {
            BlockMaterial::Solid
        } else if y < 4 {
            BlockMaterial::Water
        } else {
            BlockMaterial::Air
        }
    }
}

impl PathingProbe for Tank {
    fn can_stand_at(&self, _position: Vec3) -> PathingProbeResult {
        panic!("water navigation must not accept the land-pathing shortcut")
    }

    fn can_entity_swim_at(&self, _id: EntityId, position: Vec3) -> PathingProbeResult {
        water_occupancy(position, BOUNDS, |x, y, z| Ok(self.material_at(x, y, z)))
    }
}

fn spawn(store: &mut EntityStore, species: &str, position: Vec3) -> EntityId {
    let mut entity = SpawnEntity::new(1, species, position);
    entity.on_ground = false;
    entity.goal = GoalState::AquaticWander {
        speed: 0.7,
        vertical_speed: 0.18,
        period_ticks: 40,
    };
    store.spawn(entity)
}

fn step(store: &mut EntityStore, id: EntityId, tick: u64, config: PhysicsConfig) -> Vec3 {
    store.tick_goals_with_pathing(tick, &Tank, PathingBudget::DEFAULT);
    let snapshot = store.snapshot(id).unwrap();
    let stepped = mc_physics::step_entity(
        EntityBody {
            position: mc_physics::Vec3::new(
                snapshot.position.x,
                snapshot.position.y,
                snapshot.position.z,
            ),
            velocity: mc_physics::Vec3::new(
                snapshot.velocity.x,
                snapshot.velocity.y,
                snapshot.velocity.z,
            ),
            aabb: BOUNDS,
            on_ground: snapshot.on_ground,
        },
        &Tank,
        config,
    );
    let position = Vec3::new(
        stepped.body.position.x,
        stepped.body.position.y,
        stepped.body.position.z,
    );
    store.runtime.queue_physics(EntityPhysicsResult {
        id,
        position,
        rotation: snapshot.rotation,
        velocity: Vec3::new(
            stepped.body.velocity.x,
            stepped.body.velocity.y,
            stepped.body.velocity.z,
        ),
        on_ground: stepped.body.on_ground,
    });
    store.runtime.run_stage(EntityStage::PhysicsApply);
    position
}

#[test]
fn fish_keeps_its_heading_across_the_old_global_period_boundary() {
    let mut store = EntityStore::new();
    let id = spawn(&mut store, "minecraft:cod", Vec3::new(2.0, 2.0, 2.0));
    // No position integration: the destination has not been reached.
    for tick in 1..40 {
        store.tick_goals(tick);
    }
    let before = store.snapshot(id).unwrap();
    store.tick_goals(40);
    let after = store.snapshot(id).unwrap();
    assert_eq!(after.rotation.yaw, before.rotation.yaw);
    assert!(before.velocity.x * after.velocity.x + before.velocity.z * after.velocity.z > 0.0);
}

#[test]
fn fish_faces_forward_and_keeps_inertia_when_turning() {
    let velocity = Vec3::new(1.0, 0.0, 0.0);
    let mut rotation = Rotation {
        yaw: -90.0,
        pitch: 0.0,
        head_yaw: -90.0,
    };
    let target = Vec3::new(0.0, 0.0, 4.0);
    let (turned, _) = steer(
        Swimmer::Fish,
        EntityId(1),
        1,
        Vec3::ZERO,
        target,
        rotation,
        velocity,
        0.7,
        0.7,
    );
    face_motion(Swimmer::Fish, Vec3::ZERO, target, turned, &mut rotation);
    assert_eq!(rotation.yaw, 0.0); // +Z, not the previous erroneous +90 degrees.
    assert_eq!(turned.x, velocity.x); // Steering adds force; it does not replace momentum.
    assert!((turned.z - 0.14).abs() < 1.0e-7);
    assert_eq!(rotation.pitch, 0.0); // FishMoveControl does not drive body pitch.
}

#[test]
fn fish_and_squid_stay_in_water_and_clear_obstacles_over_many_turns() {
    for (species, config) in [
        ("minecraft:cod", PhysicsConfig::fish_entity()),
        ("minecraft:squid", PhysicsConfig::squid_entity()),
    ] {
        let mut store = EntityStore::new();
        let start = Vec3::new(3.5, 3.5, 2.5); // near both the surface and the internal wall
        let id = spawn(&mut store, species, start);
        let mut furthest: f64 = 0.0;
        for tick in 1..=600 {
            let position = step(&mut store, id, tick, config);
            assert_eq!(
                Tank.can_entity_swim_at(id, position),
                PathingProbeResult::Walkable,
                "{species} left swimmable water at tick {tick}: {position:?}"
            );
            let delta = difference(position, start);
            furthest = furthest.max(delta.horizontal_len().hypot(delta.y));
        }
        // Confinement must not be implemented by permanently freezing swimmers.
        assert!(
            furthest > 1.0,
            "{species} failed to escape its initial obstacle neighborhood"
        );
    }
}

#[test]
fn squid_coasts_then_pulses_instead_of_using_fish_acceleration() {
    let id = EntityId(3);
    let phase_speed = 0.2 / (crate::deterministic_unit(id, 0x51) + 1.0);
    let pulse_tick = (0..100)
        .find(|tick| {
            let phase = ((*tick as f64 + 1.0) * phase_speed).rem_euclid(std::f64::consts::TAU);
            phase > std::f64::consts::PI * 0.75 && phase < std::f64::consts::PI
        })
        .unwrap();
    let target = Vec3::new(3.0, 0.25, 0.0);
    let (coasting, _) = steer(
        Swimmer::Squid,
        id,
        0,
        Vec3::ZERO,
        target,
        Rotation::ZERO,
        Vec3::ZERO,
        0.0,
        0.7,
    );
    let (pulse, _) = steer(
        Swimmer::Squid,
        id,
        pulse_tick,
        Vec3::ZERO,
        target,
        Rotation::ZERO,
        coasting,
        0.0,
        0.7,
    );
    assert_eq!(coasting, Vec3::ZERO);
    assert!((pulse.x - 4.0).abs() < 1.0e-12);
    assert!((pulse.y - 0.5).abs() < 1.0e-12);
}

#[test]
fn restored_swimmer_continues_the_same_navigation_and_stroke() {
    let mut first = EntityStore::new();
    let id = spawn(&mut first, "minecraft:squid", Vec3::new(2.0, 2.0, 2.0));
    for tick in 1..20 {
        step(&mut first, id, tick, PhysicsConfig::squid_entity());
    }
    let snapshot = first.snapshot(id).unwrap();
    let mut restored = EntityStore::new();
    assert!(restored.insert_runtime_snapshot(snapshot));
    for tick in 20..60 {
        assert_eq!(
            step(&mut first, id, tick, PhysicsConfig::squid_entity()),
            step(&mut restored, id, tick, PhysicsConfig::squid_entity())
        );
        assert_eq!(
            first.snapshot(id).unwrap().velocity,
            restored.snapshot(id).unwrap().velocity
        );
    }
}

#[test]
fn missing_water_samples_do_not_create_airborne_swim_thrust() {
    struct Missing;
    impl PathingProbe for Missing {
        fn can_stand_at(&self, _position: Vec3) -> PathingProbeResult {
            PathingProbeResult::Walkable
        }
    }
    let mut store = EntityStore::new();
    let id = spawn(&mut store, "minecraft:cod", Vec3::new(2.0, 2.0, 2.0));
    store.set_velocity(id, Vec3::new(1.0, 2.0, 0.0));
    store.tick_goals_with_pathing(1, &Missing, PathingBudget::DEFAULT);
    assert_eq!(store.snapshot(id).unwrap().velocity, Vec3::ZERO);
}
