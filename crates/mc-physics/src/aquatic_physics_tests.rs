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
    // A stationary living body has no upward swim drive, so it sinks under
    // the vanilla water gravity instead of receiving surface lift.
    assert!(generic.body.velocity.y < 0.0);
}

#[test]
fn fish_move_before_drag_and_sink_while_squid_travel_does_not_double_damp() {
    let body = EntityBody {
        velocity: Vec3::new(1.0, 0.0, 0.0),
        ..stationary_body()
    };
    let fish = super::step_entity(body, &WaterWorld, PhysicsConfig::fish_entity());
    let squid = super::step_entity(body, &WaterWorld, PhysicsConfig::squid_entity());

    assert!((fish.body.position.x - 0.55).abs() < 1.0e-12);
    assert_eq!(fish.body.position.y, body.position.y);
    assert!((fish.body.velocity.x - 0.9).abs() < 1.0e-12);
    assert!((fish.body.velocity.y + 0.1).abs() < 1.0e-12);
    assert_eq!(squid.body.position, fish.body.position);
    assert_eq!(squid.body.velocity, body.velocity);
}

/// Open water with the surface at the top of `y = 63`: fluid at and below,
/// air above, no floor and no walls.
struct DeepWater;

impl BlockSampler for DeepWater {
    fn material_at(&self, _x: i32, y: i32, _z: i32) -> BlockMaterial {
        if y <= 63 {
            BlockMaterial::Water
        } else {
            BlockMaterial::Air
        }
    }
}

fn submerged_living_body() -> EntityBody {
    EntityBody {
        position: Vec3::new(0.5, 61.0, 0.5),
        velocity: Vec3::ZERO,
        aabb: Aabb::COW,
        on_ground: false,
    }
}

#[test]
fn living_bodies_stay_immersed_while_swimming() {
    // Regression: the old unconditional surface buoyancy (+7.0 blocks/s^2 on
    // any overlap) settled terrestrial bodies with their feet above the
    // surface — the owner-observed sheep/pigs walking on water. A swimming
    // body (FloatGoal-style upward drive while submerged) must never breach
    // the surface over a sustained bob.
    let mut body = submerged_living_body();
    let mut highest_feet = body.position.y;
    for _ in 0..500 {
        if body.position.y < 63.4 {
            body.velocity.y = 0.12;
        }
        body = super::step_entity(body, &DeepWater, PhysicsConfig::living_entity()).body;
        highest_feet = highest_feet.max(body.position.y);
    }

    assert!(
        highest_feet < 63.9,
        "swimming body breached the surface: feet reached {highest_feet}"
    );
}

#[test]
fn swimming_living_bodies_hold_near_breathing_depth() {
    // While the body actively swims, the bounded lift must hold it near the
    // breathing band instead of letting it sink away or park at the surface.
    let mut body = submerged_living_body();
    body.position.y = 63.0;
    for _ in 0..500 {
        if body.position.y < 63.4 {
            body.velocity.y = 0.12;
        }
        let result = super::step_entity(body, &DeepWater, PhysicsConfig::living_entity());
        body = result.body;
        assert!(result.in_fluid);
        assert!(
            (63.0..63.9).contains(&body.position.y),
            "swimming body left the breathing band: feet at {}",
            body.position.y
        );
    }
}

#[test]
fn passive_living_bodies_sink_instead_of_hovering_at_surface() {
    // Regression for the owner-observed surface hover: with no upward swim
    // drive (a passive sheep that wandered into a pond), the body must keep
    // sinking like vanilla instead of pinning just below the surface.
    let mut body = submerged_living_body();
    for _ in 0..500 {
        let result = super::step_entity(body, &DeepWater, PhysicsConfig::living_entity());
        body = result.body;
        assert!(result.in_fluid);
    }

    assert!(
        body.position.y < 55.0,
        "passive body hovered instead of sinking: feet at {}",
        body.position.y
    );
    assert!(body.velocity.y < 0.0, "passive body stopped sinking");
}

#[test]
fn item_like_bodies_keep_unconditional_surface_buoyancy() {
    // Boundary: the generic (non-living) config keeps legacy floating-item
    // behavior, so dropped items still ride up to the surface.
    let mut body = submerged_living_body();
    for _ in 0..120 {
        body = super::step_entity(body, &DeepWater, PhysicsConfig::default()).body;
    }

    assert!(
        body.position.y > 63.0,
        "item-like body did not float to the surface: feet at {}",
        body.position.y
    );
}

/// Open water against a tall solid wall at `x >= 2`, for shore-transition
/// probing.
struct WaterAgainstWall;

impl BlockSampler for WaterAgainstWall {
    fn material_at(&self, x: i32, y: i32, _z: i32) -> BlockMaterial {
        if x >= 2 && (60..=66).contains(&y) {
            BlockMaterial::Solid
        } else if y <= 63 {
            BlockMaterial::Water
        } else {
            BlockMaterial::Air
        }
    }
}

#[test]
fn swimming_body_rammed_against_shore_gets_bounded_exit_pop() {
    // The exit impulse is available near the surface, not against submerged
    // walls where the raised body would still be in liquid.
    let mut body = EntityBody {
        position: Vec3::new(1.4, 63.6, 0.5),
        velocity: Vec3::new(6.0, 0.0, 0.0),
        ..submerged_living_body()
    };
    let mut popped = false;
    for _ in 0..60 {
        let result = super::step_entity(body, &WaterAgainstWall, PhysicsConfig::living_entity());
        body = result.body;
        if result.horizontal_collision {
            popped = true;
            break;
        }
    }

    assert!(popped, "swimmer never reached the wall");
    assert!((body.velocity.y - 0.300_000_011_920_928_96 / 0.05).abs() < 1e-9);
}
