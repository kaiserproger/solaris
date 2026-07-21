use std::sync::OnceLock;

use mc_data::entity_types::{EntityTypeFacts, EntityTypeRegistry};
use mc_data::item_components::AttackRangeFacts;
use mc_entity::{AnimalBreedingState, Vec3};
use mc_physics::Aabb;
use mc_protocol::packets::play::{GameMode, unpack_block_pos};

use crate::play::PlayerPose;
use crate::play::spawn::chunk_pos_from_coords;
use crate::{MAX_VIEW_DISTANCE, MIN_VIEW_DISTANCE};

pub(in crate::play) fn entity_is_near_player_chunk(
    chunk: (i32, i32),
    player_positions: &[Vec3],
    simulation_distance: i32,
) -> bool {
    let simulation_distance =
        simulation_distance.clamp(MIN_VIEW_DISTANCE, MAX_VIEW_DISTANCE) as u32;
    player_positions.iter().any(|position| {
        let player_chunk = chunk_pos_from_coords(position.x, position.z);
        chunk.0.abs_diff(player_chunk.0) <= simulation_distance
            && chunk.1.abs_diff(player_chunk.1) <= simulation_distance
    })
}

pub(super) fn distance_sq(a: Vec3, b: Vec3) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    let dz = a.z - b.z;
    dx * dx + dy * dy + dz * dz
}

fn canonical_entity_registry() -> &'static EntityTypeRegistry {
    static REGISTRY: OnceLock<EntityTypeRegistry> = OnceLock::new();
    REGISTRY.get_or_init(mc_data::entity_types::solaris_required_entity_types)
}

pub(super) fn canonical_entity_facts(type_name: &str) -> Option<&'static EntityTypeFacts> {
    let id = mc_data::Identifier::parse(type_name.to_string()).ok()?;
    canonical_entity_registry().facts_of(&id)
}

pub(in crate::play) fn entity_aabb(type_name: &str) -> Aabb {
    let facts = canonical_entity_facts(type_name)
        .expect("entity AABB requires a canonical 26.1.2 entity type");
    Aabb {
        half_width: facts.dimensions.half_width(),
        height: facts.dimensions.height,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct EntityGeometry {
    pub(super) aabb: Aabb,
    pub(super) eye_height: f64,
}

pub(super) fn entity_geometry(
    type_name: &str,
    animal: Option<AnimalBreedingState>,
) -> EntityGeometry {
    if animal.is_some_and(AnimalBreedingState::is_baby) {
        let baby = match type_name {
            "minecraft:chicken" => Some((0.15, 0.4, 0.28)),
            "minecraft:cow" => Some((0.225, 0.7, 0.665)),
            "minecraft:pig" => Some((0.225, 0.45, 0.3825)),
            "minecraft:sheep" => Some((0.225, 0.65, 0.6175)),
            _ => None,
        };
        if let Some((half_width, height, eye_height)) = baby {
            return EntityGeometry {
                aabb: Aabb { half_width, height },
                eye_height,
            };
        }
    }
    let dimensions = canonical_entity_facts(type_name)
        .expect("entity geometry requires a canonical 26.1.2 entity type")
        .dimensions;
    EntityGeometry {
        aabb: Aabb {
            half_width: dimensions.half_width(),
            height: dimensions.height,
        },
        eye_height: dimensions.eye_height.unwrap_or(dimensions.height * 0.85),
    }
}

fn player_eye_position(pose: PlayerPose) -> Vec3 {
    Vec3::new(pose.x, pose.y + pose.eye_height(), pose.z)
}

pub(super) fn player_aabb_for_pose(pose: PlayerPose) -> Aabb {
    Aabb {
        half_width: 0.3,
        height: pose.body_height(),
    }
}

fn block_bounds(position: i64) -> (Vec3, Vec3) {
    let (x, y, z) = unpack_block_pos(position);
    (
        Vec3::new(f64::from(x), f64::from(y), f64::from(z)),
        Vec3::new(f64::from(x) + 1.0, f64::from(y) + 1.0, f64::from(z) + 1.0),
    )
}

pub(in crate::play) fn within_block_reach(
    pose: PlayerPose,
    position: i64,
    game_mode: GameMode,
) -> bool {
    // Player defaults plus ServerPlayer's packet-verification buffer.
    let max = if game_mode == GameMode::Creative {
        6.0
    } else {
        5.5
    };
    let (min, max_bound) = block_bounds(position);
    distance_sq_to_box(player_eye_position(pose), min, max_bound) < max * max
}

pub(in crate::play) fn within_entity_reach(
    pose: PlayerPose,
    position: Vec3,
    aabb: Aabb,
    game_mode: GameMode,
) -> bool {
    // Entity-interaction attribute plus ServerPlayer's verification buffer.
    let max = if game_mode == GameMode::Creative {
        8.0
    } else {
        6.0
    };
    distance_sq_to_entity_box(player_eye_position(pose), position, aabb) < max * max
}

pub(in crate::play) fn within_entity_attack_reach(
    pose: PlayerPose,
    position: Vec3,
    aabb: Aabb,
    game_mode: GameMode,
    attack_range: Option<AttackRangeFacts>,
) -> bool {
    let (min, max, margin) = if let Some(range) = attack_range {
        let (min, max) = if game_mode == GameMode::Creative {
            (range.min_creative_reach, range.max_creative_reach)
        } else {
            (range.min_reach, range.max_reach)
        };
        (
            f64::from(min),
            f64::from(max),
            f64::from(range.hitbox_margin),
        )
    } else {
        let default = if game_mode == GameMode::Creative {
            5.0
        } else {
            3.0
        };
        (0.0, default, 0.0)
    };
    let buffer = 3.0;
    let min = (min - margin - buffer).max(0.0);
    let max = max + margin + buffer;
    let distance_sq = distance_sq_to_entity_box(player_eye_position(pose), position, aabb);
    distance_sq >= min * min && distance_sq <= max * max
}

fn distance_sq_to_entity_box(point: Vec3, position: Vec3, aabb: Aabb) -> f64 {
    distance_sq_to_box(
        point,
        Vec3::new(
            position.x - aabb.half_width,
            position.y,
            position.z - aabb.half_width,
        ),
        Vec3::new(
            position.x + aabb.half_width,
            position.y + aabb.height,
            position.z + aabb.half_width,
        ),
    )
}

fn distance_sq_to_box(point: Vec3, min: Vec3, max: Vec3) -> f64 {
    if !vec3_is_finite(point) || !vec3_is_finite(min) || !vec3_is_finite(max) {
        return f64::INFINITY;
    }
    let axis_distance = |value: f64, min: f64, max: f64| {
        if value < min {
            min - value
        } else if value > max {
            value - max
        } else {
            0.0
        }
    };
    let dx = axis_distance(point.x, min.x, max.x);
    let dy = axis_distance(point.y, min.y, max.y);
    let dz = axis_distance(point.z, min.z, max.z);
    dx * dx + dy * dy + dz * dz
}

fn vec3_is_finite(value: Vec3) -> bool {
    value.x.is_finite() && value.y.is_finite() && value.z.is_finite()
}
