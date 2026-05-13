//! # mc-physics
//!
//! Block physics, collisions, fluids.
//!
//! Part of the Solaris engine.

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const TICK_SECONDS: f64 = 0.05;
pub const GRAVITY_BLOCKS_PER_SECOND_SQUARED: f64 = 32.0;
pub const TERMINAL_VELOCITY_BLOCKS_PER_SECOND: f64 = -78.4;
pub const GROUND_FRICTION: f64 = 0.6;
pub const AIR_DRAG: f64 = 0.98;
pub const WATER_DRAG: f64 = 0.8;
pub const WATER_BUOYANCY_BLOCKS_PER_SECOND_SQUARED: f64 = 7.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub half_width: f64,
    pub height: f64,
}

impl Aabb {
    pub const COW: Self = Self {
        half_width: 0.46,
        height: 1.4,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockMaterial {
    Air,
    Solid,
    Water,
    Lava,
}

impl BlockMaterial {
    #[must_use]
    pub const fn is_solid(self) -> bool {
        matches!(self, Self::Solid)
    }

    #[must_use]
    pub const fn is_fluid(self) -> bool {
        matches!(self, Self::Water | Self::Lava)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockMaterialIds {
    pub air: u32,
    pub water: Option<u32>,
    pub lava: Option<u32>,
}

impl BlockMaterialIds {
    #[must_use]
    pub const fn new(air: u32, water: Option<u32>, lava: Option<u32>) -> Self {
        Self { air, water, lava }
    }

    #[must_use]
    pub fn classify(self, state_id: u32) -> BlockMaterial {
        if state_id == self.air {
            BlockMaterial::Air
        } else if self.water == Some(state_id) {
            BlockMaterial::Water
        } else if self.lava == Some(state_id) {
            BlockMaterial::Lava
        } else {
            BlockMaterial::Solid
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityBody {
    pub position: Vec3,
    pub velocity: Vec3,
    pub aabb: Aabb,
    pub on_ground: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicsConfig {
    pub tick_seconds: f64,
    pub gravity: f64,
    pub terminal_velocity: f64,
    pub ground_friction: f64,
    pub air_drag: f64,
    pub water_drag: f64,
    pub water_buoyancy: f64,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            tick_seconds: TICK_SECONDS,
            gravity: GRAVITY_BLOCKS_PER_SECOND_SQUARED,
            terminal_velocity: TERMINAL_VELOCITY_BLOCKS_PER_SECOND,
            ground_friction: GROUND_FRICTION,
            air_drag: AIR_DRAG,
            water_drag: WATER_DRAG,
            water_buoyancy: WATER_BUOYANCY_BLOCKS_PER_SECOND_SQUARED,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepResult {
    pub body: EntityBody,
    pub in_fluid: bool,
}

pub trait BlockSampler {
    fn material_at(&mut self, x: i32, y: i32, z: i32) -> BlockMaterial;
}

pub fn step_entity<S: BlockSampler>(
    mut body: EntityBody,
    sampler: &mut S,
    config: PhysicsConfig,
) -> StepResult {
    let in_fluid = body_overlaps_fluid(body, sampler);
    if in_fluid {
        body.velocity.y += config.water_buoyancy * config.tick_seconds;
        body.velocity.x *= config.water_drag;
        body.velocity.y *= config.water_drag;
        body.velocity.z *= config.water_drag;
    } else {
        body.velocity.y -= config.gravity * config.tick_seconds;
        body.velocity.x *= config.air_drag;
        body.velocity.z *= config.air_drag;
    }
    body.velocity.y = body.velocity.y.max(config.terminal_velocity);

    body.position.x += body.velocity.x * config.tick_seconds;
    body.position.y += body.velocity.y * config.tick_seconds;
    body.position.z += body.velocity.z * config.tick_seconds;

    let ground_y = ground_y_for_body(body, sampler);
    let min_y = body.position.y;
    body.on_ground = false;
    if let Some(ground_y) = ground_y
        && min_y < ground_y
        && body.velocity.y <= 0.0
    {
        body.position.y = ground_y;
        body.velocity.y = 0.0;
        body.velocity.x *= config.ground_friction;
        body.velocity.z *= config.ground_friction;
        body.on_ground = true;
    }

    StepResult { body, in_fluid }
}

pub fn ground_y_for_body<S: BlockSampler>(body: EntityBody, sampler: &mut S) -> Option<f64> {
    let feet_y = body.position.y.floor() as i32;
    bbox_columns(body)
        .into_iter()
        .flat_map(|(x, z)| [feet_y, feet_y - 1].map(move |y| (x, y, z)))
        .filter(|&(x, y, z)| sampler.material_at(x, y, z).is_solid())
        .map(|(_, y, _)| f64::from(y + 1))
        .max_by(f64::total_cmp)
}

fn body_overlaps_fluid<S: BlockSampler>(body: EntityBody, sampler: &mut S) -> bool {
    let min_y = body.position.y.floor() as i32;
    let max_y = (body.position.y + body.aabb.height).floor() as i32;
    bbox_columns(body)
        .into_iter()
        .any(|(x, z)| (min_y..=max_y).any(|y| sampler.material_at(x, y, z).is_fluid()))
}

fn bbox_columns(body: EntityBody) -> [(i32, i32); 4] {
    let x = body.position.x;
    let z = body.position.z;
    let half = body.aabb.half_width;
    [
        ((x - half).floor() as i32, (z - half).floor() as i32),
        ((x - half).floor() as i32, (z + half).floor() as i32),
        ((x + half).floor() as i32, (z - half).floor() as i32),
        ((x + half).floor() as i32, (z + half).floor() as i32),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FlatWorld {
        ground_y: i32,
        water_y: Option<i32>,
    }

    impl BlockSampler for FlatWorld {
        fn material_at(&mut self, _x: i32, y: i32, _z: i32) -> BlockMaterial {
            if self.water_y == Some(y) {
                BlockMaterial::Water
            } else if y <= self.ground_y {
                BlockMaterial::Solid
            } else {
                BlockMaterial::Air
            }
        }
    }

    #[test]
    fn gravity_lands_body_on_solid_ground() {
        let mut world = FlatWorld {
            ground_y: 63,
            water_y: None,
        };
        let mut body = EntityBody {
            position: Vec3::new(0.5, 66.0, 0.5),
            velocity: Vec3::ZERO,
            aabb: Aabb::COW,
            on_ground: false,
        };

        for _ in 0..40 {
            body = step_entity(body, &mut world, PhysicsConfig::default()).body;
        }

        assert_eq!(body.position.y, 64.0);
        assert_eq!(body.velocity.y, 0.0);
        assert!(body.on_ground);
    }

    #[test]
    fn air_drag_damps_horizontal_motion() {
        let mut world = FlatWorld {
            ground_y: -64,
            water_y: None,
        };
        let body = EntityBody {
            position: Vec3::new(0.5, 70.0, 0.5),
            velocity: Vec3::new(1.0, 0.0, 0.0),
            aabb: Aabb::COW,
            on_ground: false,
        };

        let stepped = step_entity(body, &mut world, PhysicsConfig::default()).body;

        assert!(stepped.velocity.x < body.velocity.x);
        assert!(stepped.velocity.y < 0.0);
    }

    #[test]
    fn water_applies_buoyancy_and_stronger_drag() {
        let mut world = FlatWorld {
            ground_y: 60,
            water_y: Some(64),
        };
        let body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::new(1.0, -1.0, 0.0),
            aabb: Aabb::COW,
            on_ground: false,
        };

        let result = step_entity(body, &mut world, PhysicsConfig::default());

        assert!(result.in_fluid);
        assert!(result.body.velocity.x < 1.0);
        assert!(result.body.velocity.y > -1.0);
    }

    #[test]
    fn material_ids_classify_fluids_apart_from_solids() {
        let ids = BlockMaterialIds::new(0, Some(5), Some(6));

        assert_eq!(ids.classify(0), BlockMaterial::Air);
        assert_eq!(ids.classify(5), BlockMaterial::Water);
        assert_eq!(ids.classify(6), BlockMaterial::Lava);
        assert_eq!(ids.classify(7), BlockMaterial::Solid);
    }
}
