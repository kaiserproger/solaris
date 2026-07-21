use super::{Aabb, BlockMaterial, BlockSampler, EntityBody, PhysicsConfig, Vec3};

struct WaterWorld;

impl BlockSampler for WaterWorld {
    fn material_at(&self, _x: i32, _y: i32, _z: i32) -> BlockMaterial {
        BlockMaterial::Water
    }
}

fn stationary_body() -> EntityBody {
    EntityBody {
        position: Vec3::new(0.5, 62.0, 0.5),
        velocity: Vec3::ZERO,
        aabb: Aabb {
            half_width: 0.3,
            height: 0.6,
        },
        on_ground: false,
    }
}

#[test]
fn aquatic_entities_do_not_receive_generic_surface_buoyancy() {
    let aquatic = super::step_entity(
        stationary_body(),
        &WaterWorld,
        PhysicsConfig::aquatic_entity(),
    );
    let generic = super::step_entity(
        stationary_body(),
        &WaterWorld,
        PhysicsConfig::living_entity(),
    );

    assert_eq!(aquatic.body.velocity.y, 0.0);
    assert!(generic.body.velocity.y > 0.0);
}

#[test]
fn aquatic_entities_keep_vanilla_fish_water_drag() {
    let body = EntityBody {
        velocity: Vec3::new(1.0, 0.0, 0.0),
        ..stationary_body()
    };
    let result = super::step_entity(body, &WaterWorld, PhysicsConfig::aquatic_entity());

    assert!((result.body.velocity.x - 0.9).abs() < f64::EPSILON);
}
