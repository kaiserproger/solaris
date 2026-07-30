use std::collections::HashMap;

use mc_entity::{EntityLifecycle, EntitySimulationProjection, SpawnEntity, Vec3};
use mc_physics::{Aabb, BlockMaterial, BlockMaterialIds};
use mc_world::light::ChunkLight;
use mc_world::{BlockPos, ChunkPos, WorldReadSnapshot};

use crate::play::{
    HerdSpawn, MAX_HOSTILE_SPAWNS_PER_CHUNK, MAX_PASSIVE_SPAWNS_PER_CHUNK,
    MIN_ENTITY_SPAWN_DISTANCE_FROM_PLAYER, herd_uuid, is_hostile_entity,
};

use super::super::entity_goal_defaults::apply_default_mob_goal;
use super::super::entity_physics_class::entity_type_uses_aquatic_physics;
use super::super::entity_spawn_facts::apply_entity_facts;
use super::super::interaction_geometry::{distance_sq, entity_aabb};
use super::scheduler::{NaturalSpawnCategory, NaturalSpawnCategoryReport};
use super::{VANILLA_CREATURE_MOB_CAP, VANILLA_HOSTILE_MOB_CAP, VANILLA_WATER_CREATURE_MOB_CAP};

pub(super) const MAX_NATURAL_TEMPLATES_PER_CHUNK: usize = 16;

#[derive(Debug, Clone, Copy)]
pub(super) struct NaturalSpawnCapacities {
    hostile: usize,
    ground: usize,
    aquatic: usize,
}

impl NaturalSpawnCapacities {
    pub(super) fn from_counts(hostile: usize, ground: usize, aquatic: usize) -> Self {
        Self {
            hostile: VANILLA_HOSTILE_MOB_CAP.saturating_sub(hostile),
            ground: VANILLA_CREATURE_MOB_CAP.saturating_sub(ground),
            aquatic: VANILLA_WATER_CREATURE_MOB_CAP.saturating_sub(aquatic),
        }
    }

    fn admit(&mut self, type_name: &str) -> bool {
        let remaining = if is_hostile_entity(type_name) {
            &mut self.hostile
        } else if entity_type_uses_aquatic_physics(type_name) {
            &mut self.aquatic
        } else {
            &mut self.ground
        };
        if *remaining == 0 {
            return false;
        }
        *remaining -= 1;
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpawnTerrainRejection {
    Unloaded,
    BlockOrFluid,
    Darkness,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn plan_periodic_category(
    category: NaturalSpawnCategory,
    chunks: &[(i32, i32)],
    templates: &HashMap<(i32, i32), Vec<HerdSpawn>>,
    world_snapshot: Option<&WorldReadSnapshot>,
    materials: Option<&BlockMaterialIds>,
    player_positions: &[Vec3],
    projections: &[EntitySimulationProjection],
    accepted_boxes: &mut Vec<(Vec3, Aabb)>,
    capacities: &mut NaturalSpawnCapacities,
    tick: u64,
    nighttime: bool,
    mob_behaviors: &mc_data::mob_behavior_26_1_2::MobBehaviorTable,
) -> (NaturalSpawnCategoryReport, Vec<SpawnEntity>) {
    let mut report = NaturalSpawnCategoryReport {
        chunks_sampled: chunks.len() as u64,
        ..NaturalSpawnCategoryReport::default()
    };
    let mut planned = Vec::new();
    for &chunk in chunks {
        let Some(chunk_templates) = templates.get(&chunk) else {
            report.rejected_unloaded = report.rejected_unloaded.saturating_add(1);
            continue;
        };
        let per_chunk_cap = match category {
            NaturalSpawnCategory::Friendly => MAX_PASSIVE_SPAWNS_PER_CHUNK,
            NaturalSpawnCategory::Hostile => MAX_HOSTILE_SPAWNS_PER_CHUNK,
        };
        let mut accepted_in_chunk = 0_usize;
        for template in chunk_templates
            .iter()
            .take(MAX_NATURAL_TEMPLATES_PER_CHUNK)
            .filter(|template| template.hostile == (category == NaturalSpawnCategory::Hostile))
        {
            report.templates_considered = report.templates_considered.saturating_add(1);
            if category == NaturalSpawnCategory::Hostile && !nighttime {
                report.rejected_time = report.rejected_time.saturating_add(1);
                continue;
            }
            if accepted_in_chunk >= per_chunk_cap {
                report.rejected_cap = report.rejected_cap.saturating_add(1);
                continue;
            }
            if !spawn_far_enough_from_players(
                player_positions,
                template.position,
                MIN_ENTITY_SPAWN_DISTANCE_FROM_PLAYER,
            ) {
                report.rejected_player_distance = report.rejected_player_distance.saturating_add(1);
                continue;
            }
            match periodic_spawn_terrain_admission(category, template, world_snapshot, materials) {
                Ok(()) => {}
                Err(SpawnTerrainRejection::Unloaded) => {
                    report.rejected_unloaded = report.rejected_unloaded.saturating_add(1);
                    continue;
                }
                Err(SpawnTerrainRejection::BlockOrFluid) => {
                    report.rejected_block_or_fluid =
                        report.rejected_block_or_fluid.saturating_add(1);
                    continue;
                }
                Err(SpawnTerrainRejection::Darkness) => {
                    report.rejected_darkness = report.rejected_darkness.saturating_add(1);
                    continue;
                }
            }
            let candidate_box = entity_aabb(&template.entity_type_name);
            if projections.iter().any(|entity| {
                entity.lifecycle == EntityLifecycle::Alive
                    && entity_aabbs_intersect(
                        template.position,
                        candidate_box,
                        entity.position,
                        entity_aabb(&entity.type_name),
                    )
            }) || accepted_boxes.iter().any(|&(position, aabb)| {
                entity_aabbs_intersect(template.position, candidate_box, position, aabb)
            }) {
                report.rejected_collision = report.rejected_collision.saturating_add(1);
                continue;
            }
            if !capacities.admit(&template.entity_type_name) {
                report.rejected_cap = report.rejected_cap.saturating_add(1);
                continue;
            }
            let Some(mut candidate) = build_herd_spawn_candidates(
                chunk,
                std::slice::from_ref(template),
                player_positions,
                tick,
                MIN_ENTITY_SPAWN_DISTANCE_FROM_PLAYER,
                mob_behaviors,
            )
            .pop() else {
                report.rejected_duplicate_or_stale =
                    report.rejected_duplicate_or_stale.saturating_add(1);
                continue;
            };
            candidate.uuid = Some(periodic_herd_uuid(template, tick));
            accepted_in_chunk += 1;
            accepted_boxes.push((template.position, candidate_box));
            planned.push(candidate);
        }
    }
    (report, planned)
}

fn periodic_herd_uuid(template: &HerdSpawn, tick: u64) -> uuid::Uuid {
    let base = herd_uuid(template.chunk, template.slot).as_u128();
    let attempt = u128::from(tick)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .rotate_left(47);
    uuid::Uuid::from_u128(base ^ attempt)
}

fn periodic_spawn_terrain_admission(
    category: NaturalSpawnCategory,
    template: &HerdSpawn,
    world_snapshot: Option<&WorldReadSnapshot>,
    materials: Option<&BlockMaterialIds>,
) -> Result<(), SpawnTerrainRejection> {
    let snapshot = world_snapshot.ok_or(SpawnTerrainRejection::Unloaded)?;
    let materials = materials.ok_or(SpawnTerrainRejection::Unloaded)?;
    let position = template.position;
    let aabb = entity_aabb(&template.entity_type_name);
    let min_x = (position.x - aabb.half_width + f64::EPSILON).floor() as i32;
    let max_x = (position.x + aabb.half_width - f64::EPSILON).floor() as i32;
    let min_y = position.y.floor() as i32;
    let max_y = (position.y + aabb.height - f64::EPSILON).floor() as i32;
    let min_z = (position.z - aabb.half_width + f64::EPSILON).floor() as i32;
    let max_z = (position.z + aabb.half_width - f64::EPSILON).floor() as i32;
    let aquatic = entity_type_uses_aquatic_physics(&template.entity_type_name);

    if !aquatic {
        let support = snapshot
            .get_cached_block(BlockPos {
                x: position.x.floor() as i32,
                y: min_y.saturating_sub(1),
                z: position.z.floor() as i32,
            })
            .ok_or(SpawnTerrainRejection::Unloaded)?;
        if !materials.classify(support.0).is_solid() {
            return Err(SpawnTerrainRejection::BlockOrFluid);
        }
    }

    for x in min_x..=max_x {
        for z in min_z..=max_z {
            for y in min_y..=max_y {
                let state = snapshot
                    .get_cached_block(BlockPos { x, y, z })
                    .ok_or(SpawnTerrainRejection::Unloaded)?;
                let material = materials.classify(state.0);
                if (aquatic && material != BlockMaterial::Water)
                    || (!aquatic && material != BlockMaterial::Air)
                {
                    return Err(SpawnTerrainRejection::BlockOrFluid);
                }
            }
        }
    }

    if category == NaturalSpawnCategory::Hostile {
        let chunk_pos = ChunkPos {
            x: min_x.div_euclid(mc_world::SECTION_DIM as i32),
            z: min_z.div_euclid(mc_world::SECTION_DIM as i32),
        };
        let chunk = snapshot
            .chunk(chunk_pos)
            .ok_or(SpawnTerrainRejection::Unloaded)?;
        let light = ChunkLight::from_chunk(&chunk).ok_or(SpawnTerrainRejection::Darkness)?;
        let local_x = min_x.rem_euclid(mc_world::SECTION_DIM as i32) as u8;
        let local_z = min_z.rem_euclid(mc_world::SECTION_DIM as i32) as u8;
        if light.block_at(local_x, min_y, local_z) != 0 {
            return Err(SpawnTerrainRejection::Darkness);
        }
    }
    Ok(())
}

fn entity_aabbs_intersect(
    left_position: Vec3,
    left: Aabb,
    right_position: Vec3,
    right: Aabb,
) -> bool {
    left_position.x - left.half_width < right_position.x + right.half_width
        && right_position.x - right.half_width < left_position.x + left.half_width
        && left_position.y < right_position.y + right.height
        && right_position.y < left_position.y + left.height
        && left_position.z - left.half_width < right_position.z + right.half_width
        && right_position.z - right.half_width < left_position.z + left.half_width
}

pub(super) fn build_herd_spawn_candidates(
    chunk: (i32, i32),
    spawns: &[HerdSpawn],
    player_positions: &[Vec3],
    lifecycle_tick: u64,
    minimum_player_distance: f64,
    mob_behaviors: &mc_data::mob_behavior_26_1_2::MobBehaviorTable,
) -> Vec<SpawnEntity> {
    let mut passive_count = 0_usize;
    let mut hostile_count = 0_usize;
    let mut entities = Vec::new();
    for spawn in spawns {
        debug_assert_eq!(spawn.chunk, chunk);
        if spawn.hostile {
            if hostile_count >= MAX_HOSTILE_SPAWNS_PER_CHUNK {
                continue;
            }
        } else if passive_count >= MAX_PASSIVE_SPAWNS_PER_CHUNK {
            continue;
        }
        if !spawn_far_enough_from_players(player_positions, spawn.position, minimum_player_distance)
        {
            continue;
        }
        let mut entity = SpawnEntity::new(
            spawn.entity_type_id,
            spawn.entity_type_name.clone(),
            spawn.position,
        );
        entity.retained.spawn_tick = lifecycle_tick;
        entity.uuid = Some(herd_uuid(spawn.chunk, spawn.slot));
        apply_entity_facts(&mut entity);
        if let Some(color) = spawn.sheep_color {
            debug_assert_eq!(entity.type_name, "minecraft:sheep");
            entity.animal = Some(mc_entity::AnimalBreedingState::adult_sheep(color));
        }
        apply_default_mob_goal(&mut entity, mob_behaviors);
        entities.push(entity);
        if spawn.hostile {
            hostile_count += 1;
        } else {
            passive_count += 1;
        }
    }
    entities
}

pub(in crate::play::session) fn spawn_far_enough_from_players(
    player_positions: &[Vec3],
    position: Vec3,
    minimum_distance: f64,
) -> bool {
    let min_distance_sq = minimum_distance * minimum_distance;
    player_positions
        .iter()
        .all(|player| distance_sq(position, *player) > min_distance_sq)
}
