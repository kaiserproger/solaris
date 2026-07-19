use mc_data::Identifier;
use mc_data::entity_types::{
    EntityDimensions, fallback_entity_dimensions, fallback_entity_type_facts,
};
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

pub(in crate::play) fn entity_aabb(type_name: &str) -> Aabb {
    let facts = Identifier::parse(type_name.to_string())
        .map(|id| fallback_entity_type_facts(id, 0))
        .ok();
    facts.map_or(Aabb::COW, |facts| Aabb {
        half_width: facts.dimensions.half_width(),
        height: facts.dimensions.height,
    })
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
    let dimensions =
        fallback_entity_dimensions(type_name, animal.is_some_and(AnimalBreedingState::is_baby))
            .or_else(|| {
                Identifier::parse(type_name.to_string())
                    .ok()
                    .map(|id| fallback_entity_type_facts(id, 0).dimensions)
            })
            .unwrap_or(EntityDimensions::new(
                Aabb::COW.half_width * 2.0,
                Aabb::COW.height,
                None,
            ));
    EntityGeometry {
        aabb: Aabb {
            half_width: dimensions.half_width(),
            height: dimensions.height,
        },
        eye_height: dimensions.eye_height.unwrap_or(dimensions.height * 0.85),
    }
}

fn player_eye_position(pose: PlayerPose) -> Vec3 {
    Vec3::new(pose.x, pose.y + 1.62, pose.z)
}

fn block_center(position: i64) -> Vec3 {
    let (x, y, z) = unpack_block_pos(position);
    Vec3::new(x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5)
}

pub(in crate::play) fn within_block_reach(
    pose: PlayerPose,
    position: i64,
    game_mode: GameMode,
) -> bool {
    let max = if game_mode == GameMode::Creative {
        6.0
    } else {
        5.0
    };
    distance_sq(player_eye_position(pose), block_center(position)) <= max * max
}

pub(in crate::play) fn within_entity_reach(
    pose: PlayerPose,
    position: Vec3,
    aabb: Aabb,
    game_mode: GameMode,
) -> bool {
    let max = if game_mode == GameMode::Creative {
        6.0
    } else {
        5.0
    };
    distance_sq_to_entity_box(player_eye_position(pose), position, aabb) <= max * max
}

fn distance_sq_to_entity_box(point: Vec3, position: Vec3, aabb: Aabb) -> f64 {
    let dx = (point.x - position.x).abs() - aabb.half_width;
    let dz = (point.z - position.z).abs() - aabb.half_width;
    let dy = if point.y < position.y {
        position.y - point.y
    } else if point.y > position.y + aabb.height {
        point.y - (position.y + aabb.height)
    } else {
        0.0
    };
    dx.max(0.0).powi(2) + dy.powi(2) + dz.max(0.0).powi(2)
}
