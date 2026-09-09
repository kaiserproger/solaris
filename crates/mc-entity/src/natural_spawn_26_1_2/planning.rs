use mc_physics::Aabb;
use mc_world::{BlockStateId, Chunk};

use crate::{
    SpawnEntity, Vec3,
    natural_spawn_26_1_2::{
        HerdSpawn, MAX_HOSTILE_SPAWNS_PER_CHUNK, MAX_PASSIVE_SPAWNS_PER_CHUNK,
        apply_default_mob_goal, apply_entity_facts, choose_biome_spawn, entity_aabb,
        herd_entry_count, herd_hash, herd_uuid, hostile_chunk_spawns, natural_sheep_color,
        safe_land_spawn_offset,
    },
};

#[cfg(test)]
#[path = "planning_tests.rs"]
mod tests;

#[cfg(test)]
mod hostile_template_tests {
    use mc_data::Identifier;
    use mc_world::{BlockStateId, Chunk, ChunkPos};

    use super::{LandSpawnSurfaces, hostile_spawn_positions};

    #[test]
    fn hostile_template_positions_include_enclosed_cave_and_surface() {
        let air = BlockStateId(0);
        let stone = BlockStateId(1);
        let plains = Identifier::parse("minecraft:plains").unwrap();
        let mut chunk = Chunk::empty(ChunkPos { x: 0, z: 0 }, air, plains);
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block(x, 40, z, stone).unwrap();
                chunk.set_block(x, 43, z, stone).unwrap();
                chunk.set_block(x, 64, z, stone).unwrap();
            }
        }

        let positions = hostile_spawn_positions(
            &chunk,
            LandSpawnSurfaces {
                preferred: stone,
                fallbacks: &[],
            },
            &[air],
            0x4341_5645_5F51_4100,
        );

        assert!(
            positions.iter().any(|(_, y, _)| *y < 64),
            "expected an underground hostile template position: {positions:?}"
        );
        assert!(
            positions.iter().any(|(_, y, _)| *y == 65),
            "expected a surface fallback hostile template position: {positions:?}"
        );
    }
}

pub(super) fn entity_aabbs_intersect(
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
    let mut spawns = Vec::new();
    if let Some(surface) = land_surface {
        let surfaces = LandSpawnSurfaces {
            preferred: surface,
            fallbacks: land_fallback_surfaces,
        };
        plan_group_spawns(
            chunk,
            surfaces,
            passable,
            "creature",
            rules,
            entity_types,
            &mut spawns,
        );
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
    let positions = hostile_spawn_positions(chunk, surfaces, passable, h);
    let Some(&(lx, spawn_y, lz)) = positions.first() else {
        return;
    };
    let Some(biome) = chunk_biome_at(chunk, lx, spawn_y, lz) else {
        return;
    };
    let Some(entry) = choose_biome_spawn(rules.entries(biome, "monster"), chunk_pos, slot_base)
    else {
        return;
    };
    if !entity_type_is_hostile(entity_types, &entry.entity_type) {
        return;
    }
    let Some(entity_type_id) = entity_types
        .id_of(&entry.entity_type)
        .and_then(|id| i32::try_from(id).ok())
    else {
        return;
    };
    let count = herd_entry_count(entry, chunk_pos, slot_base).min(6);
    let initial_len = out.len();
    let bounds = entity_aabb(entry.entity_type.as_str());
    for (lx, spawn_y, lz) in positions {
        if out.len() - initial_len >= count {
            break;
        }
        if chunk_biome_at(chunk, lx, spawn_y, lz) != Some(biome) {
            continue;
        }
        let slot = out.len() as u8;
        let offset = herd_hash(chunk_pos, slot, 0x484F_5354_494C_4500);
        let position = Vec3::new(
            f64::from(chunk_pos.0 * 16 + i32::from(lx)) + safe_land_spawn_offset(offset),
            f64::from(spawn_y),
            f64::from(chunk_pos.1 * 16 + i32::from(lz)) + safe_land_spawn_offset(offset >> 2),
        );
        if out.iter().any(|member| {
            entity_aabbs_intersect(
                position,
                bounds,
                member.position,
                entity_aabb(&member.entity_type_name),
            )
        }) {
            continue;
        }
        out.push(HerdSpawn {
            chunk: chunk_pos,
            slot,
            entity_type_id,
            entity_type_name: entry.entity_type.as_str().to_string(),
            position,
            hostile: true,
            sheep_color: None,
        });
    }
}

fn hostile_spawn_positions(
    chunk: &Chunk,
    surfaces: LandSpawnSurfaces<'_>,
    passable: &[BlockStateId],
    seed: u64,
) -> Vec<(u8, i32, u8)> {
    const MAX_POSITIONS: usize = 6;
    const MAX_CAVE_POSITIONS: usize = 2;
    const CAVE_COLUMN_PROBES: u64 = 12;

    let geometry = chunk.geometry();
    let min_spawn_y = geometry.min_y().saturating_add(1);
    let mut positions = Vec::with_capacity(MAX_POSITIONS);

    for column_attempt in 0..CAVE_COLUMN_PROBES {
        let candidate = seed.wrapping_add(column_attempt.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let lx = 1 + (candidate as u8 % 14);
        let lz = 1 + ((candidate >> 8) as u8 % 14);
        let Some(surface_y) = (geometry.min_y()..geometry.max_y()).rev().find(|&y| {
            chunk
                .get_block(lx, y, lz)
                .is_some_and(|state| !passable.contains(&state))
        }) else {
            continue;
        };
        let max_spawn_y = surface_y
            .saturating_sub(1)
            .min(geometry.max_y().saturating_sub(2));
        if max_spawn_y < min_spawn_y {
            continue;
        }
        let span = u64::try_from(max_spawn_y - min_spawn_y + 1).unwrap_or(1);
        let start = (candidate >> 16) % span;
        for offset in 0..span {
            let spawn_y = min_spawn_y + i32::try_from((start + offset) % span).unwrap_or(0);
            let position = (lx, spawn_y, lz);
            if hostile_land_spawn_cell_clear(chunk, lx, spawn_y, lz, passable)
                && !positions.contains(&position)
            {
                positions.push(position);
                break;
            }
        }
        if positions.len() >= MAX_CAVE_POSITIONS {
            break;
        }
    }

    for surface_attempt in 0..16_u64 {
        if positions.len() >= MAX_POSITIONS {
            break;
        }
        let surface_seed = seed
            .wrapping_add(0x5355_5246_4143_4500)
            .wrapping_add(surface_attempt.wrapping_mul(0xC2B2_AE3D_27D4_EB4F));
        let Some((lx, support_y, lz)) = herd_spawn_surface(chunk, surfaces, passable, surface_seed)
        else {
            continue;
        };
        let position = (lx, support_y + 1, lz);
        if !positions.contains(&position) {
            positions.push(position);
        }
    }

    positions
}

fn hostile_land_spawn_cell_clear(
    chunk: &Chunk,
    lx: u8,
    spawn_y: i32,
    lz: u8,
    passable: &[BlockStateId],
) -> bool {
    chunk
        .get_block(lx, spawn_y - 1, lz)
        .is_some_and(|support| !passable.contains(&support))
        && (spawn_y..=spawn_y + 1).all(|y| {
            chunk
                .get_block(lx, y, lz)
                .is_some_and(|state| passable.contains(&state))
        })
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
    // Leave room between herd members while keeping support probes local.
    // Extra candidates replace members rejected at shorelines and chunk edges.
    let initial_len = out.len();
    let spacing = rules.placement().land_spacing() as i8;
    for (dx, dz) in [
        (0_i8, 0_i8),
        (1, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (-1, 1),
        (1, -1),
        (-1, -1),
    ] {
        if out.len() - initial_len >= count {
            break;
        }
        let Some(member_x) = lx.checked_add_signed(dx * spacing).filter(|x| *x < 16) else {
            continue;
        };
        let Some(member_z) = lz.checked_add_signed(dz * spacing).filter(|z| *z < 16) else {
            continue;
        };
        let Some((member_y, actual_surface)) = (y - 4..=y + 4).rev().find_map(|candidate_y| {
            let state = chunk.get_block(member_x, candidate_y, member_z)?;
            (state == surfaces.preferred || surfaces.fallbacks.contains(&state))
                .then_some((candidate_y, state))
        }) else {
            continue;
        };
        if !herd_spawn_minimal_clearance(
            chunk,
            member_x,
            member_y + 1,
            member_z,
            actual_surface,
            passable,
        ) || chunk_biome_at(chunk, member_x, member_y, member_z) != Some(biome)
        {
            continue;
        }
        let position = Vec3::new(
            f64::from(chunk.pos.x * 16 + i32::from(member_x)) + 0.5,
            f64::from(member_y + 1),
            f64::from(chunk.pos.z * 16 + i32::from(member_z)) + 0.5,
        );
        let bounds = entity_aabb(entry.entity_type.as_str());
        if out.iter().any(|member| {
            entity_aabbs_intersect(
                position,
                bounds,
                member.position,
                entity_aabb(&member.entity_type_name),
            )
        }) {
            continue;
        }
        let slot = out.len() as u8;
        out.push(HerdSpawn {
            chunk: chunk_pos,
            slot,
            entity_type_id,
            entity_type_name: entry.entity_type.as_str().to_string(),
            position,
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
    let placement = rules.placement();
    let mut positions = (0..u64::from(placement.water_attempts())).filter_map(|attempt| {
        let hash = herd_hash(chunk_pos, slot_base, 0x5741_5445_5200_0000 ^ attempt);
        let x = hash as u8 % 16;
        let z = (hash >> 8) as u8 % 16;
        water_spawn_y(chunk, x, z, water, sea_level, placement.water_depth()).map(|y| (x, y, z))
    });
    let Some(leader @ (lx, y, lz)) = positions.next() else {
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
    let bounds = entity_aabb(entry.entity_type.as_str());
    let initial_len = out.len();
    for (x, y, z) in std::iter::once(leader).chain(positions) {
        if out.len() - initial_len >= count {
            break;
        }
        if chunk_biome_at(chunk, x, y, z) != Some(biome) {
            continue;
        }
        let position = Vec3::new(
            f64::from(chunk.pos.x * 16 + i32::from(x)) + 0.5,
            f64::from(y),
            f64::from(chunk.pos.z * 16 + i32::from(z)) + 0.5,
        );
        if out.iter().any(|member| {
            entity_aabbs_intersect(
                position,
                bounds,
                member.position,
                entity_aabb(&member.entity_type_name),
            )
        }) {
            continue;
        }
        out.push(HerdSpawn {
            chunk: chunk_pos,
            slot: out.len() as u8,
            entity_type_id,
            entity_type_name: entry.entity_type.as_str().to_string(),
            position,
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
    depth: u8,
) -> Option<i32> {
    let is_water = |y| {
        chunk
            .get_block(lx, y, lz)
            .is_some_and(|state| water.contains(&state))
    };
    let top = (mc_world::MIN_Y..=sea_level).rev().find(|&y| is_water(y))?;
    let mut spawn_y = top;
    // The startup rule selects the initial water band; execution stays native.
    for y in (top - i32::from(depth)..top).rev() {
        if !is_water(y) {
            break;
        }
        spawn_y = y;
    }
    Some(spawn_y)
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
