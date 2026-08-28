use std::collections::HashMap;

use mc_physics::{Aabb, BlockMaterial, BlockMaterialIds};
use mc_world::light::ChunkLight;
use mc_world::{BlockPos, BlockStateId, Chunk, ChunkPos, WorldReadSnapshot};

use crate::{
    EntityLifecycle, EntitySimulationProjection, SpawnEntity, Vec3,
    natural_spawn_26_1_2::{
        HerdSpawn, MAX_HOSTILE_SPAWNS_PER_CHUNK, MAX_PASSIVE_SPAWNS_PER_CHUNK,
        MIN_ENTITY_SPAWN_DISTANCE_FROM_PLAYER, VANILLA_CREATURE_MOB_CAP, VANILLA_HOSTILE_MOB_CAP,
        VANILLA_WATER_CREATURE_MOB_CAP, apply_default_mob_goal, apply_entity_facts,
        choose_biome_spawn, entity_aabb, entity_type_uses_aquatic_physics, herd_entry_count,
        herd_hash, herd_uuid, hostile_chunk_spawns, is_hostile_entity, natural_sheep_color,
        passive_chunk_spawns, safe_land_spawn_offset,
    },
};

use super::scheduler::{NaturalSpawnCategory, NaturalSpawnCategoryReport};

pub const MAX_NATURAL_TEMPLATES_PER_CHUNK: usize = 16;

#[derive(Debug, Clone, Copy)]
pub struct NaturalSpawnCapacities {
    hostile: usize,
    ground: usize,
    aquatic: usize,
}

impl NaturalSpawnCapacities {
    pub fn from_counts(hostile: usize, ground: usize, aquatic: usize) -> Self {
        Self {
            hostile: VANILLA_HOSTILE_MOB_CAP.saturating_sub(hostile),
            ground: VANILLA_CREATURE_MOB_CAP.saturating_sub(ground),
            aquatic: VANILLA_WATER_CREATURE_MOB_CAP.saturating_sub(aquatic),
        }
    }

    fn admit(&mut self, spawn: &HerdSpawn) -> bool {
        let remaining = if is_hostile_entity(&spawn.entity_type_name) {
            &mut self.hostile
        } else if entity_type_uses_aquatic_physics(&spawn.entity_type_name) {
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
pub fn plan_periodic_category(
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
            if !capacities.admit(template) {
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

pub fn build_herd_spawn_candidates(
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
            entity.animal = Some(crate::AnimalBreedingState::adult_sheep(color));
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

pub fn spawn_far_enough_from_players(
    player_positions: &[Vec3],
    position: Vec3,
    minimum_distance: f64,
) -> bool {
    let min_distance_sq = minimum_distance * minimum_distance;
    player_positions
        .iter()
        .all(|player| distance_sq(position, *player) > min_distance_sq)
}

fn distance_sq(a: Vec3, b: Vec3) -> f64 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    let dz = a.z - b.z;
    dx * dx + dy * dy + dz * dz
}

#[derive(Clone, Copy)]
struct LandSpawnSurfaces<'a> {
    preferred: BlockStateId,
    fallbacks: &'a [BlockStateId],
}

pub struct ChunkHerdPlanningContext<'a> {
    pub land_surface: Option<BlockStateId>,
    pub land_fallback_surfaces: &'a [BlockStateId],
    pub water: Option<&'a [BlockStateId]>,
    pub passable: &'a [BlockStateId],
    pub sea_level: i32,
}

#[must_use]
pub fn plan_chunk_herd_templates(
    chunk: &Chunk,
    context: ChunkHerdPlanningContext<'_>,
    rules: &mc_data::biomes::BiomeSpawnRules,
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
) -> Vec<HerdSpawn> {
    let ChunkHerdPlanningContext {
        land_surface,
        land_fallback_surfaces,
        water,
        passable,
        sea_level,
    } = context;
    let chunk_pos = (chunk.pos.x, chunk.pos.z);
    let mut spawns = Vec::new();
    if let Some(surface) = land_surface {
        let surfaces = LandSpawnSurfaces {
            preferred: surface,
            fallbacks: land_fallback_surfaces,
        };
        if passive_chunk_spawns(chunk_pos) {
            plan_group_spawns(
                chunk,
                surfaces,
                passable,
                "creature",
                rules,
                entity_types,
                &mut spawns,
            );
        }
        plan_hostile_spawns(chunk, surfaces, passable, rules, entity_types, &mut spawns);
    }
    if let Some(water) = water.filter(|states| !states.is_empty()) {
        plan_water_group_spawns(
            chunk,
            water,
            "water_ambient",
            rules,
            entity_types,
            sea_level,
            &mut spawns,
        );
        plan_water_group_spawns(
            chunk,
            water,
            "water_creature",
            rules,
            entity_types,
            sea_level,
            &mut spawns,
        );
    }
    spawns
}

fn plan_hostile_spawns(
    chunk: &Chunk,
    surfaces: LandSpawnSurfaces<'_>,
    passable: &[BlockStateId],
    rules: &mc_data::biomes::BiomeSpawnRules,
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
    out: &mut Vec<HerdSpawn>,
) {
    let chunk_pos = (chunk.pos.x, chunk.pos.z);
    if !hostile_chunk_spawns(chunk_pos) {
        return;
    }
    let slot_base = out.len() as u8;
    let h = herd_hash(chunk_pos, slot_base, 0x5A4F_4D42_4945_0000);
    let Some((lx, y, lz)) = herd_spawn_surface(chunk, surfaces, passable, h) else {
        return;
    };
    let Some(biome) = chunk_biome_at(chunk, lx, y, lz) else {
        return;
    };
    for (hostile_index, entry) in rules
        .entries(biome, "monster")
        .iter()
        .filter(|entry| entity_type_is_hostile(entity_types, &entry.entity_type))
        .take(3)
        .enumerate()
    {
        let Some(entity_type_id) = entity_types
            .id_of(&entry.entity_type)
            .and_then(|id| i32::try_from(id).ok())
        else {
            continue;
        };
        let slot = slot_base + hostile_index as u8;
        let offset = herd_hash(chunk_pos, slot, 0x484F_5354_494C_4500);
        out.push(HerdSpawn {
            chunk: chunk_pos,
            slot,
            entity_type_id,
            entity_type_name: entry.entity_type.as_str().to_string(),
            position: Vec3::new(
                f64::from(chunk.pos.x * 16 + i32::from(lx)) + safe_land_spawn_offset(offset),
                f64::from(y + 1),
                f64::from(chunk.pos.z * 16 + i32::from(lz)) + safe_land_spawn_offset(offset >> 2),
            ),
            hostile: true,
            sheep_color: None,
        });
    }
}

fn entity_type_is_hostile(
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
    entity_type: &mc_data::Identifier,
) -> bool {
    entity_types
        .facts_of(entity_type)
        .is_some_and(|facts| facts.category.is_hostile())
}

fn plan_group_spawns(
    chunk: &Chunk,
    surfaces: LandSpawnSurfaces<'_>,
    passable: &[BlockStateId],
    group: &str,
    rules: &mc_data::biomes::BiomeSpawnRules,
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
    out: &mut Vec<HerdSpawn>,
) {
    let chunk_pos = (chunk.pos.x, chunk.pos.z);
    let slot_base = out.len() as u8;
    let h = herd_hash(chunk_pos, slot_base, 0x5350_4157_4E00_0000);
    let Some((lx, y, lz)) = herd_spawn_surface(chunk, surfaces, passable, h) else {
        return;
    };
    let Some(biome) = chunk_biome_at(chunk, lx, y, lz) else {
        return;
    };
    let Some(entry) = choose_biome_spawn(rules.entries(biome, group), chunk_pos, slot_base) else {
        return;
    };
    let Some(entity_type_id) = entity_types
        .id_of(&entry.entity_type)
        .and_then(|id| i32::try_from(id).ok())
    else {
        return;
    };
    let count = herd_entry_count(entry, chunk_pos, slot_base).min(6);
    for i in 0..count {
        let slot = slot_base + i as u8;
        let offset = herd_hash(chunk_pos, slot, 0x4F46_4653_4554_0000);
        out.push(HerdSpawn {
            chunk: chunk_pos,
            slot,
            entity_type_id,
            entity_type_name: entry.entity_type.as_str().to_string(),
            position: Vec3::new(
                f64::from(chunk.pos.x * 16 + i32::from(lx)) + safe_land_spawn_offset(offset),
                f64::from(y + 1),
                f64::from(chunk.pos.z * 16 + i32::from(lz)) + safe_land_spawn_offset(offset >> 2),
            ),
            hostile: false,
            sheep_color: (entry.entity_type.as_str() == "minecraft:sheep")
                .then(|| natural_sheep_color(rules.sheep_color_climate(biome), chunk_pos, slot)),
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_water_group_spawns(
    chunk: &Chunk,
    water: &[BlockStateId],
    group: &str,
    rules: &mc_data::biomes::BiomeSpawnRules,
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
    sea_level: i32,
    out: &mut Vec<HerdSpawn>,
) {
    let chunk_pos = (chunk.pos.x, chunk.pos.z);
    let slot_base = out.len() as u8;
    let h = herd_hash(chunk_pos, slot_base, 0x5741_5445_5200_0000);
    let lx = 3 + (h as u8 % 10);
    let lz = 3 + ((h >> 8) as u8 % 10);
    let Some(spawn_y) = water_spawn_y(chunk, lx, lz, water, sea_level) else {
        return;
    };
    let Some(biome) = chunk_biome_at(chunk, lx, spawn_y, lz) else {
        return;
    };
    let Some(entry) = choose_biome_spawn(rules.entries(biome, group), chunk_pos, slot_base) else {
        return;
    };
    let Some(entity_type_id) = entity_types
        .id_of(&entry.entity_type)
        .and_then(|id| i32::try_from(id).ok())
    else {
        return;
    };
    let count = herd_entry_count(entry, chunk_pos, slot_base).min(6);
    for i in 0..count {
        let slot = slot_base + i as u8;
        out.push(HerdSpawn {
            chunk: chunk_pos,
            slot,
            entity_type_id,
            entity_type_name: entry.entity_type.as_str().to_string(),
            position: Vec3::new(
                f64::from(chunk.pos.x * 16 + i32::from(lx)) + 0.5,
                f64::from(spawn_y),
                f64::from(chunk.pos.z * 16 + i32::from(lz)) + 0.5,
            ),
            hostile: false,
            sheep_color: None,
        });
    }
}

fn water_spawn_y(
    chunk: &Chunk,
    lx: u8,
    lz: u8,
    water: &[BlockStateId],
    sea_level: i32,
) -> Option<i32> {
    let mut best_run = None;
    let mut current_start = None;
    for y in mc_world::MIN_Y..=sea_level {
        if chunk
            .get_block(lx, y, lz)
            .is_some_and(|state| water.contains(&state))
        {
            current_start.get_or_insert(y);
            continue;
        }
        if let Some(start) = current_start.take() {
            remember_water_run(&mut best_run, start, y - 1);
        }
    }
    if let Some(start) = current_start.take() {
        remember_water_run(&mut best_run, start, sea_level);
    }
    best_run.map(|(start, end)| start + (end - start) / 2)
}

fn remember_water_run(best_run: &mut Option<(i32, i32)>, start: i32, end: i32) {
    let len = end - start;
    if best_run
        .map(|(best_start, best_end)| len > best_end - best_start)
        .unwrap_or(true)
    {
        *best_run = Some((start, end));
    }
}

pub fn chunk_biome_at(chunk: &Chunk, lx: u8, y: i32, lz: u8) -> Option<&mc_data::Identifier> {
    let geometry = chunk.geometry();
    if !(geometry.min_y()..geometry.max_y()).contains(&y) {
        return None;
    }
    let chunk_y = (y - geometry.min_y()) as usize;
    let section = chunk.biomes.get(chunk_y / mc_world::SECTION_DIM)?;
    let local_y = (chunk_y % mc_world::SECTION_DIM) as u8 / mc_world::BIOME_DIM as u8;
    Some(section.get(lx / 4, local_y, lz / 4))
}

pub fn herd_surface_y(
    chunk: &Chunk,
    lx: u8,
    lz: u8,
    surface: BlockStateId,
    fallback_surfaces: &[BlockStateId],
    passable: &[BlockStateId],
) -> Option<(i32, BlockStateId)> {
    if let Some(y) = chunk.highest_opaque_y(lx, lz)
        && chunk.get_block(lx, y, lz) == Some(surface)
    {
        return Some((y, surface));
    }
    if let Some(y) = (mc_world::MIN_Y..mc_world::MAX_Y)
        .rev()
        .find(|&y| chunk.get_block(lx, y, lz) == Some(surface))
    {
        return Some((y, surface));
    }
    herd_land_surface_y(chunk, lx, lz, fallback_surfaces, passable)
}

fn herd_spawn_surface(
    chunk: &Chunk,
    surfaces: LandSpawnSurfaces<'_>,
    passable: &[BlockStateId],
    h: u64,
) -> Option<(u8, i32, u8)> {
    for attempt in 0..100u64 {
        let candidate = h.wrapping_add(attempt.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let lx = 3 + (candidate as u8 % 10);
        let lz = 3 + ((candidate >> 8) as u8 % 10);
        let Some((y, actual_surface)) = herd_surface_y(
            chunk,
            lx,
            lz,
            surfaces.preferred,
            surfaces.fallbacks,
            passable,
        ) else {
            continue;
        };
        if herd_spawn_clearance(chunk, lx, y + 1, lz, actual_surface, passable) {
            return Some((lx, y, lz));
        }
    }
    for attempt in 0..100u64 {
        let candidate = h.wrapping_add(attempt.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let lx = 3 + (candidate as u8 % 10);
        let lz = 3 + ((candidate >> 8) as u8 % 10);
        let Some((y, actual_surface)) = herd_surface_y(
            chunk,
            lx,
            lz,
            surfaces.preferred,
            surfaces.fallbacks,
            passable,
        ) else {
            continue;
        };
        if herd_spawn_minimal_clearance(chunk, lx, y + 1, lz, actual_surface, passable) {
            return Some((lx, y, lz));
        }
    }
    None
}

fn herd_land_surface_y(
    chunk: &Chunk,
    lx: u8,
    lz: u8,
    fallback_surfaces: &[BlockStateId],
    passable: &[BlockStateId],
) -> Option<(i32, BlockStateId)> {
    let y = chunk.highest_opaque_y(lx, lz)?;
    let state = chunk.get_block(lx, y, lz)?;
    if passable.contains(&state) || !fallback_surfaces.contains(&state) {
        return None;
    }
    if (y + 1..=y + 2).all(|air_y| {
        chunk
            .get_block(lx, air_y, lz)
            .is_some_and(|state| passable.contains(&state))
    }) {
        Some((y, state))
    } else {
        None
    }
}

fn herd_spawn_clearance(
    chunk: &Chunk,
    lx: u8,
    spawn_y: i32,
    lz: u8,
    surface: BlockStateId,
    passable: &[BlockStateId],
) -> bool {
    for dx in -1..=1 {
        for dz in -1..=1 {
            let x = i32::from(lx) + dx;
            let z = i32::from(lz) + dz;
            if !(0..mc_world::SECTION_DIM as i32).contains(&x)
                || !(0..mc_world::SECTION_DIM as i32).contains(&z)
            {
                return false;
            }
            let x = x as u8;
            let z = z as u8;
            if chunk.get_block(x, spawn_y - 1, z) != Some(surface) {
                return false;
            }
            if !(spawn_y..=spawn_y + 1).all(|y| {
                chunk
                    .get_block(x, y, z)
                    .is_some_and(|state| passable.contains(&state))
            }) {
                return false;
            }
        }
    }
    true
}

fn herd_spawn_minimal_clearance(
    chunk: &Chunk,
    lx: u8,
    spawn_y: i32,
    lz: u8,
    surface: BlockStateId,
    passable: &[BlockStateId],
) -> bool {
    if chunk.get_block(lx, spawn_y - 1, lz) != Some(surface) {
        return false;
    }
    if !(spawn_y..=spawn_y + 1).all(|y| {
        chunk
            .get_block(lx, y, lz)
            .is_some_and(|state| passable.contains(&state))
    }) {
        return false;
    }
    [(1, 0), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .any(|(dx, dz)| {
            let x = i32::from(lx) + dx;
            let z = i32::from(lz) + dz;
            (0..mc_world::SECTION_DIM as i32).contains(&x)
                && (0..mc_world::SECTION_DIM as i32).contains(&z)
                && chunk.get_block(x as u8, spawn_y - 1, z as u8) == Some(surface)
                && (spawn_y..=spawn_y + 1).all(|y| {
                    chunk
                        .get_block(x as u8, y, z as u8)
                        .is_some_and(|state| passable.contains(&state))
                })
        })
}
