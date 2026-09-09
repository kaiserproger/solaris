use std::collections::HashMap;

use mc_physics::{Aabb, BlockMaterial, BlockMaterialIds};
use mc_world::light::ChunkLight;
use mc_world::{BlockPos, ChunkPos, WorldReadSnapshot};

use crate::{EntityLifecycle, EntitySimulationProjection, SpawnEntity, Vec3};

use super::planning::{
    build_herd_spawn_candidates, entity_aabbs_intersect, spawn_far_enough_from_players,
};
use super::scheduler::{NaturalSpawnCategory, NaturalSpawnCategoryReport};
use super::{
    HerdSpawn, MIN_ENTITY_SPAWN_DISTANCE_FROM_PLAYER, entity_aabb,
    entity_type_uses_aquatic_physics, herd_hash, herd_uuid,
};

pub const MAX_NATURAL_TEMPLATES_PER_CHUNK: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpawnTerrainRejection {
    Unloaded,
    BlockOrFluid,
    Time,
    Darkness,
}

#[allow(clippy::too_many_arguments)]
pub fn plan_periodic_category(
    category: NaturalSpawnCategory,
    chunks: &[(i32, i32)],
    templates: &HashMap<(i32, i32), Vec<HerdSpawn>>,
    world_snapshot: Option<&WorldReadSnapshot>,
    materials: Option<&BlockMaterialIds>,
    player_positions: &[Vec3],
    projections: &[EntitySimulationProjection],
    accepted_boxes: &mut Vec<(Vec3, Aabb)>,
    tick: u64,
    world_time: u64,
    thundering: bool,
    mob_behaviors: &mc_data::mob_behavior_26_1_2::MobBehaviorTable,
) -> (NaturalSpawnCategoryReport, Vec<SpawnEntity>) {
    let mut report = NaturalSpawnCategoryReport {
        chunks_sampled: chunks.len() as u64,
        ..NaturalSpawnCategoryReport::default()
    };
    let mut planned = Vec::new();
    let mut resident_boxes = Vec::new();
    for &chunk in chunks {
        let Some(chunk_templates) = templates.get(&chunk) else {
            report.rejected_unloaded = report.rejected_unloaded.saturating_add(1);
            continue;
        };
        for template in chunk_templates
            .iter()
            .take(MAX_NATURAL_TEMPLATES_PER_CHUNK)
            .filter(|template| template.hostile == (category == NaturalSpawnCategory::Hostile))
        {
            report.templates_considered = report.templates_considered.saturating_add(1);
            if !spawn_far_enough_from_players(
                player_positions,
                template.position,
                MIN_ENTITY_SPAWN_DISTANCE_FROM_PLAYER,
            ) {
                report.rejected_player_distance = report.rejected_player_distance.saturating_add(1);
                continue;
            }
            match periodic_spawn_terrain_admission(
                category,
                template,
                world_snapshot,
                materials,
                world_time,
                tick,
                thundering,
            ) {
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
                Err(SpawnTerrainRejection::Time) => {
                    report.rejected_time = report.rejected_time.saturating_add(1);
                    continue;
                }
                Err(SpawnTerrainRejection::Darkness) => {
                    report.rejected_darkness = report.rejected_darkness.saturating_add(1);
                    continue;
                }
            }
            let candidate_box = entity_aabb(&template.entity_type_name);
            // Memoize by projection index without changing collision short-circuiting.
            resident_boxes.resize(projections.len(), None);
            if projections
                .iter()
                .zip(&mut resident_boxes)
                .any(|(entity, aabb)| {
                    entity.lifecycle == EntityLifecycle::Alive
                        && entity_aabbs_intersect(
                            template.position,
                            candidate_box,
                            entity.position,
                            *aabb.get_or_insert_with(|| entity_aabb(&entity.type_name)),
                        )
                })
                || accepted_boxes.iter().any(|&(position, aabb)| {
                    entity_aabbs_intersect(template.position, candidate_box, position, aabb)
                })
            {
                report.rejected_collision = report.rejected_collision.saturating_add(1);
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
    world_time: u64,
    attempt_tick: u64,
    thundering: bool,
) -> Result<(), SpawnTerrainRejection> {
    let snapshot = world_snapshot.ok_or(SpawnTerrainRejection::Unloaded)?;
    let materials = materials.ok_or(SpawnTerrainRejection::Unloaded)?;
    let position = template.position;
    let spawn_block_x = position.x.floor() as i32;
    let spawn_block_z = position.z.floor() as i32;
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
                x: spawn_block_x,
                y: min_y.saturating_sub(1),
                z: spawn_block_z,
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
            x: spawn_block_x.div_euclid(mc_world::SECTION_DIM as i32),
            z: spawn_block_z.div_euclid(mc_world::SECTION_DIM as i32),
        };
        let chunk = snapshot
            .chunk(chunk_pos)
            .ok_or(SpawnTerrainRejection::Unloaded)?;
        let light = ChunkLight::from_chunk(&chunk).ok_or(SpawnTerrainRejection::Darkness)?;
        let local_x = spawn_block_x.rem_euclid(mc_world::SECTION_DIM as i32) as u8;
        let local_z = spawn_block_z.rem_euclid(mc_world::SECTION_DIM as i32) as u8;
        let sky = light.sky_at(local_x, min_y, local_z);
        let block = light.block_at(local_x, min_y, local_z);
        let sky_roll = (herd_hash(
            template.chunk,
            template.slot,
            attempt_tick ^ 0x534B_595F_524F_4C4C,
        ) & 31) as u8;
        if sky > sky_roll {
            return Err(SpawnTerrainRejection::Time);
        }
        if block > 0 {
            return Err(SpawnTerrainRejection::Darkness);
        }
        let sky_darken = if thundering {
            10
        } else {
            overworld_sky_darken_26_1_2(world_time)
        };
        let raw_brightness = block.max(sky.saturating_sub(sky_darken));
        let spawn_light_roll = (herd_hash(
            template.chunk,
            template.slot,
            attempt_tick ^ 0x4C49_4748_545F_524F,
        ) & 7) as u8;
        if raw_brightness > spawn_light_roll {
            return Err(SpawnTerrainRejection::Darkness);
        }
    }
    Ok(())
}

fn overworld_sky_darken_26_1_2(world_time: u64) -> u8 {
    let tick = (world_time % 24_000) as f32;
    let factor = if (133.0..=11_867.0).contains(&tick) {
        1.0
    } else if (11_867.0..13_670.0).contains(&tick) {
        lerp_timeline(tick, 11_867.0, 13_670.0, 1.0, 0.266_666_68)
    } else if (13_670.0..=22_330.0).contains(&tick) {
        0.266_666_68
    } else {
        let wrapped_tick = if tick < 133.0 { tick + 24_000.0 } else { tick };
        lerp_timeline(wrapped_tick, 22_330.0, 24_133.0, 0.266_666_68, 1.0)
    };
    (15.0 - 15.0 * factor) as u8
}

fn lerp_timeline(tick: f32, start: f32, end: f32, from: f32, to: f32) -> f32 {
    let progress = (tick - start) / (end - start);
    from + (to - from) * progress
}

#[cfg(test)]
#[path = "periodic_tests.rs"]
mod tests;
