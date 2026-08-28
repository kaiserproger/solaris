use std::sync::OnceLock;

use mc_data::entity_types::{
    EntityTypeFacts, EntityTypeRegistry, PhysicalSimulationClass,
    entity_type_contract_26_1_2_by_name,
};
use mc_data::mob_behavior_26_1_2::{MobBehaviorTable, MobMovementPolicy};
use mc_physics::Aabb;

use crate::{AnimalBreedingState, AttributeKind, GoalState, SheepColor, SpawnEntity, Vec3};

mod planning;
mod scheduler;

pub use planning::{
    ChunkHerdPlanningContext, MAX_NATURAL_TEMPLATES_PER_CHUNK, NaturalSpawnCapacities,
    build_herd_spawn_candidates, chunk_biome_at, herd_surface_y, plan_chunk_herd_templates,
    plan_periodic_category, spawn_far_enough_from_players,
};
pub use scheduler::{
    NaturalSpawnCategory, NaturalSpawnCategoryReport, NaturalSpawnReport, NaturalSpawnScheduler,
};

pub const MAX_PASSIVE_SPAWNS_PER_CHUNK: usize = 6;
pub const MAX_HOSTILE_SPAWNS_PER_CHUNK: usize = 3;
pub const MIN_ENTITY_SPAWN_DISTANCE_FROM_PLAYER: f64 = 24.0;
pub const VANILLA_HOSTILE_MOB_CAP: usize = 70;
pub const VANILLA_CREATURE_MOB_CAP: usize = 10;
pub const VANILLA_WATER_CREATURE_MOB_CAP: usize = 20;

#[derive(Debug, Clone, PartialEq)]
pub struct HerdSpawn {
    pub chunk: (i32, i32),
    pub slot: u8,
    pub entity_type_id: i32,
    pub entity_type_name: String,
    pub position: Vec3,
    pub hostile: bool,
    pub sheep_color: Option<SheepColor>,
}

#[must_use]
pub fn herd_hash(chunk: (i32, i32), slot: u8, salt: u64) -> u64 {
    let mut hash = salt;
    hash ^= (chunk.0 as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    hash = hash.rotate_left(23);
    hash ^= (chunk.1 as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    hash = hash.rotate_left(17);
    hash ^= (slot as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    hash ^= hash >> 30;
    hash = hash.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    hash ^= hash >> 27;
    hash.wrapping_mul(0x94D0_49BB_1331_11EB) ^ (hash >> 31)
}

#[must_use]
pub fn herd_uuid(chunk: (i32, i32), slot: u8) -> uuid::Uuid {
    let hi = herd_hash(chunk, slot, 0x434F_575F_4845_5244);
    let lo = herd_hash(chunk, slot, 0x5041_5353_4956_4500);
    uuid::Uuid::from_u128(((hi as u128) << 64) | lo as u128)
}

#[must_use]
pub fn passive_chunk_spawns(chunk: (i32, i32)) -> bool {
    chunk == (0, 0) || herd_hash(chunk, 0, 0x4845_5244).is_multiple_of(9)
}

#[must_use]
pub fn hostile_chunk_spawns(chunk: (i32, i32)) -> bool {
    chunk == (0, 0) || herd_hash(chunk, 0, 0x484F_5354_494C_4500).is_multiple_of(8)
}

#[must_use]
pub fn sheep_color_for_rolls(
    climate: mc_data::biomes::SheepColorClimate,
    outer_roll: u32,
    common_roll: u32,
) -> SheepColor {
    use mc_data::biomes::SheepColorClimate;

    debug_assert!(outer_roll < 100);
    debug_assert!(common_roll < 500);
    let common = |default| {
        if common_roll < 499 {
            default
        } else {
            SheepColor::Pink
        }
    };
    match climate {
        SheepColorClimate::Temperate => match outer_roll {
            0..=4 => SheepColor::Black,
            5..=9 => SheepColor::Gray,
            10..=14 => SheepColor::LightGray,
            15..=17 => SheepColor::Brown,
            _ => common(SheepColor::White),
        },
        SheepColorClimate::Warm => match outer_roll {
            0..=4 => SheepColor::Gray,
            5..=9 => SheepColor::LightGray,
            10..=14 => SheepColor::White,
            15..=17 => SheepColor::Black,
            _ => common(SheepColor::Brown),
        },
        SheepColorClimate::Cold => match outer_roll {
            0..=4 => SheepColor::LightGray,
            5..=9 => SheepColor::Gray,
            10..=14 => SheepColor::White,
            15..=17 => SheepColor::Brown,
            _ => common(SheepColor::Black),
        },
    }
}

#[must_use]
pub fn natural_sheep_color(
    climate: mc_data::biomes::SheepColorClimate,
    chunk: (i32, i32),
    slot: u8,
) -> SheepColor {
    let outer_roll = (herd_hash(chunk, slot, 0x5348_4545_505F_434C) % 100) as u32;
    let common_roll = (herd_hash(chunk, slot, 0x5049_4E4B_5F52_4F4C) % 500) as u32;
    sheep_color_for_rolls(climate, outer_roll, common_roll)
}

#[must_use]
pub fn choose_biome_spawn(
    entries: &[mc_data::biomes::BiomeSpawnEntry],
    chunk: (i32, i32),
    slot: u8,
) -> Option<&mc_data::biomes::BiomeSpawnEntry> {
    let total: u32 = entries.iter().map(|entry| entry.weight).sum();
    if total == 0 {
        return None;
    }
    let mut pick = (herd_hash(chunk, slot, 0x5745_4947_4854_0000) % u64::from(total)) as u32;
    for entry in entries {
        if pick < entry.weight {
            return Some(entry);
        }
        pick -= entry.weight;
    }
    entries.last()
}

#[must_use]
pub fn herd_entry_count(
    entry: &mc_data::biomes::BiomeSpawnEntry,
    chunk: (i32, i32),
    slot: u8,
) -> usize {
    let min = entry.min_count.min(entry.max_count).max(1);
    let max = entry.max_count.max(min);
    let span = max - min + 1;
    (min + (herd_hash(chunk, slot, 0x434F_554E_5400_0000) as u32 % span)) as usize
}

#[must_use]
pub fn safe_land_spawn_offset(bits: u64) -> f64 {
    0.48 + (bits & 3) as f64 * 0.01
}

#[must_use]
pub fn entity_type_facts(type_name: &str) -> Option<&'static EntityTypeFacts> {
    static REGISTRY: OnceLock<EntityTypeRegistry> = OnceLock::new();
    let id = mc_data::Identifier::parse(type_name.to_owned()).ok()?;
    REGISTRY
        .get_or_init(mc_data::entity_types::solaris_required_entity_types)
        .facts_of(&id)
}

#[must_use]
pub fn entity_aabb(type_name: &str) -> Aabb {
    let facts =
        entity_type_facts(type_name).expect("entity AABB requires a canonical 26.1.2 entity type");
    Aabb {
        half_width: facts.dimensions.half_width(),
        height: facts.dimensions.height,
    }
}

#[must_use]
pub fn entity_type_uses_aquatic_physics(type_name: &str) -> bool {
    entity_type_contract_26_1_2_by_name(type_name).is_some_and(|contract| {
        matches!(
            contract.behavior.physical_simulation,
            PhysicalSimulationClass::LivingAquatic | PhysicalSimulationClass::LivingAmphibious
        )
    })
}

#[must_use]
pub fn is_hostile_entity(type_name: &str) -> bool {
    let Some(facts) = entity_type_facts(type_name) else {
        return false;
    };
    facts.category.is_hostile()
}

pub fn apply_entity_facts(entity: &mut SpawnEntity) {
    let Some(facts) = entity_type_facts(&entity.type_name) else {
        return;
    };
    if let Some(value) = facts.attributes.max_health {
        entity.attributes.set_base(AttributeKind::MaxHealth, value);
    }
    if let Some(value) = facts.attributes.movement_speed {
        entity
            .attributes
            .set_base(AttributeKind::MovementSpeed, value);
    }
    if let Some(value) = facts.attributes.follow_range {
        entity
            .attributes
            .set_base(AttributeKind::FollowRange, value);
    }
    if let Some(value) = facts.attributes.attack_damage {
        entity
            .attributes
            .set_base(AttributeKind::AttackDamage, value);
    }
    match entity.type_name.as_str() {
        "minecraft:sheep" => {
            entity.animal = Some(AnimalBreedingState::adult_sheep(SheepColor::White));
        }
        "minecraft:cow" | "minecraft:chicken" => {
            entity.animal = Some(AnimalBreedingState::adult());
        }
        "minecraft:ender_dragon" => {
            // Vanilla clients derive the eight multipart entity ids as parent+1..+8.
            // Keep those ids unavailable to ordinary server entities.
            entity.reserved_following_ids = entity.reserved_following_ids.max(8);
        }
        _ => {}
    }
}

pub fn apply_default_mob_goal(entity: &mut SpawnEntity, behaviors: &MobBehaviorTable) {
    let Some(profile) = behaviors.get_by_name(&entity.type_name) else {
        entity.goal = GoalState::Idle;
        return;
    };
    entity.goal = match profile.movement {
        MobMovementPolicy::Immobile => GoalState::Idle,
        MobMovementPolicy::AquaticWander | MobMovementPolicy::AmphibiousWander => {
            entity.on_ground = false;
            GoalState::AquaticWander {
                speed: profile.wander_speed * 0.9,
                vertical_speed: 0.18,
                period_ticks: profile.wander_period_ticks.max(20),
            }
        }
        MobMovementPolicy::HostilePursuit => GoalState::Wander {
            speed: profile.wander_speed,
            period_ticks: profile.wander_period_ticks,
        },
        MobMovementPolicy::GroundWander
        | MobMovementPolicy::FlyingWander
        | MobMovementPolicy::VillagerSchedule => GoalState::Wander {
            speed: if profile.wander_speed > 0.0 {
                profile.wander_speed
            } else {
                passive_ground_wander_speed(entity)
            },
            period_ticks: profile.wander_period_ticks,
        },
    };
}

#[must_use]
pub fn passive_ground_wander_speed(entity: &SpawnEntity) -> f64 {
    entity
        .attributes
        .base(&AttributeKind::MovementSpeed)
        .unwrap_or(0.2)
        * 10.0
}
