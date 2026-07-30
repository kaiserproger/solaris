use std::collections::HashSet;

use mc_entity::{
    AttributeKind, EntityId, EntityLifecycle, EntitySimulationProjection, EntitySnapshot,
    GoalState, SpawnEntity, Vec3,
};
use mc_physics::{BlockMaterial, BlockMaterialIds};
use mc_world::{BlockPos, WorldReadView};

use crate::play::simulation::SimulationAuthority;

use super::entity_combat::{attack_server_entity_locked, entity_kill_rewards_locked};
use super::entity_lifecycle::track_entity_chunk_locked;
use super::interaction_geometry::{distance_sq, entity_aabb};
#[cfg(test)]
use super::outbound::OutboundCommand;
use super::outbound::VisibilityDispatch;
#[cfg(test)]
use super::visibility::spawn_entity_visibility_locked;
use super::visibility::{
    entity_event_dispatches_locked, initialize_entity_wire_state_from_snapshot_locked,
    install_committed_entity_publications_locked, server_entity_snapshot_from,
};
use super::{
    EntityAttackOutcome, SessionRegistry, apply_entity_facts, apply_entity_velocity_locked,
    is_hostile_entity, record_entity_dispatches_locked,
};

// Exact local 26.1.2 Villager/VillagerPanicTrigger/GolemSensor constants.
const VILLAGE_DEFENSE_TICK_INTERVAL: u64 = 100;
const GOLEM_SENSOR_INTERVAL: u64 = 200;
const PANIC_AGREEMENT_COUNT: usize = 3;
const VILLAGER_AGREEMENT_RANGE: f64 = 10.0;
const GOLEM_DETECTION_RANGE: f64 = 16.0;
const GOLEM_SPAWN_ATTEMPTS: usize = 10;
const GOLEM_SPAWN_HORIZONTAL_RANGE: i32 = 8;
const GOLEM_SPAWN_VERTICAL_RANGE: i32 = 6;
const GOLEM_ATTACK_INTERVAL: u64 = 20;
const GOLEM_ATTACK_EVENT: i8 = 4;
const GOLEM_VERTICAL_KNOCKBACK: f64 = 0.400_000_005_960_464_5;
const GOLEM_WANDER_SPEED: f64 = 0.6;
const GOLEM_WANDER_PERIOD_TICKS: u32 = 80;
const GOLEM_PURSUIT_SPEED: f64 = 1.0;
const MAX_GOLEM_SPAWNS_PER_TICK: usize = 4;
const DEFAULT_ATTACK_REACH: f64 = 0.828_285_658_836_771_8;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct VillageDefenseReport {
    pub(crate) spawned_golems: usize,
    pub(crate) golem_attacks: usize,
}

#[derive(Debug, Clone)]
struct PlannedGolemSpawn {
    position: Vec3,
    villagers_to_notify: Vec<EntityId>,
    uuid: uuid::Uuid,
}

#[derive(Debug, Clone, Copy)]
struct PlannedGolemAttack {
    golem_id: EntityId,
    target_id: EntityId,
    damage: f32,
}

impl SessionRegistry {
    pub(in crate::play) fn tick_village_defense(
        &self,
        _authority: &SimulationAuthority,
        tick: u64,
        iron_golem_type_id: i32,
        world_read: Option<&WorldReadView>,
        materials: Option<&BlockMaterialIds>,
    ) -> (VillageDefenseReport, Vec<VisibilityDispatch>) {
        let active_ids = self.active_simulation_entities.load_full();
        if active_ids.is_empty() {
            return (VillageDefenseReport::default(), Vec::new());
        }

        let projections = self
            .lock_entities("project village defence candidates")
            .simulation_projections_for_ids(&active_ids);

        let mut report = VillageDefenseReport::default();
        let mut dispatches = Vec::new();
        if tick.is_multiple_of(GOLEM_SENSOR_INTERVAL) {
            commit_villager_golem_memory(
                self,
                villagers_detecting_nearby_golems(&projections),
                tick,
            );
        }
        if tick.is_multiple_of(VILLAGE_DEFENSE_TICK_INTERVAL) {
            let spawns = plan_golem_spawns(
                &projections,
                tick,
                world_read,
                materials,
                MAX_GOLEM_SPAWNS_PER_TICK,
            );
            for plan in spawns {
                if let Some(spawn_dispatches) =
                    commit_golem_spawn(self, plan, tick, iron_golem_type_id)
                {
                    report.spawned_golems += 1;
                    dispatches.extend(spawn_dispatches);
                }
            }
        }

        let (goal_updates, attacks) = plan_golem_combat(&projections, tick);
        if !goal_updates.is_empty() {
            let mut entities = self.lock_entities("commit iron golem goals");
            let _ = entities.set_goals_deferred_journal(goal_updates);
        }
        for attack in attacks {
            let Some(mut attack_dispatches) = commit_golem_attack(self, attack) else {
                continue;
            };
            report.golem_attacks += 1;
            dispatches.append(&mut attack_dispatches);
        }

        (report, dispatches)
    }
}

fn plan_golem_spawns(
    projections: &[EntitySimulationProjection],
    tick: u64,
    world_read: Option<&WorldReadView>,
    materials: Option<&BlockMaterialIds>,
    max_spawns: usize,
) -> Vec<PlannedGolemSpawn> {
    let villagers = projections
        .iter()
        .filter(|entity| is_adult_villager(entity))
        .collect::<Vec<_>>();
    let golems = projections
        .iter()
        .filter(|entity| {
            entity.lifecycle == EntityLifecycle::Alive && entity.type_name == "minecraft:iron_golem"
        })
        .collect::<Vec<_>>();
    let threats = projections
        .iter()
        .filter(|entity| villager_threat_distance(&entity.type_name).is_some())
        .collect::<Vec<_>>();

    let (Some(world_read), Some(materials)) = (world_read, materials) else {
        return Vec::new();
    };

    let mut eligible = villagers
        .iter()
        .copied()
        .filter(|villager| villager_wants_golem(villager, tick))
        .filter(|villager| villager_has_nearby_threat(villager, &threats))
        .collect::<Vec<_>>();
    eligible.sort_unstable_by_key(|villager| villager.id);

    let mut consumed = HashSet::new();
    let mut spawns = Vec::new();
    for initiator in &eligible {
        if spawns.len() >= max_spawns || consumed.contains(&initiator.id) {
            continue;
        }
        let agreeing = eligible
            .iter()
            .copied()
            .filter(|villager| !consumed.contains(&villager.id))
            .filter(|villager| villagers_within_agreement_box(initiator, villager))
            .take(5)
            .collect::<Vec<_>>();
        if agreeing.len() < PANIC_AGREEMENT_COUNT {
            continue;
        }
        let nearby_villagers = villagers
            .iter()
            .copied()
            .filter(|villager| villagers_within_agreement_box(initiator, villager))
            .map(|villager| villager.id)
            .collect::<Vec<_>>();
        if golems.iter().any(|golem| {
            distance_sq(golem.position, initiator.position)
                <= GOLEM_DETECTION_RANGE * GOLEM_DETECTION_RANGE
        }) {
            consumed.extend(agreeing.into_iter().map(|villager| villager.id));
            continue;
        }
        let Some(position) = find_golem_spawn_position(
            initiator.position,
            initiator.id,
            tick,
            world_read,
            materials,
            projections,
        ) else {
            continue;
        };
        consumed.extend(agreeing.iter().map(|villager| villager.id));
        let uuid = deterministic_golem_uuid(initiator.id, tick, position);
        spawns.push(PlannedGolemSpawn {
            position,
            villagers_to_notify: nearby_villagers,
            uuid,
        });
    }
    spawns
}

fn commit_golem_spawn(
    registry: &SessionRegistry,
    plan: PlannedGolemSpawn,
    tick: u64,
    iron_golem_type_id: i32,
) -> Option<Vec<VisibilityDispatch>> {
    let mut entity = SpawnEntity::new(iron_golem_type_id, "minecraft:iron_golem", plan.position);
    entity.uuid = Some(plan.uuid);
    entity.retained.spawn_tick = tick;
    entity.goal = GoalState::Wander {
        speed: GOLEM_WANDER_SPEED,
        period_ticks: GOLEM_WANDER_PERIOD_TICKS,
    };
    apply_entity_facts(&mut entity);

    let committed = {
        let mut entities = registry.lock_entities("spawn village iron golem");
        let committed = entities.spawn_unique_batch([entity]);
        if committed.is_empty() {
            return None;
        }
        committed
    };
    commit_villager_golem_memory(registry, plan.villagers_to_notify, tick);
    let current = registry.current_expected_entity_snapshots(committed);
    if current.is_empty() {
        return None;
    }

    let mut inner = registry.lock_inner("publish village iron golem");
    let mut publications = Vec::with_capacity(current.len());
    for entity in current {
        let aabb = entity_aabb(&entity.type_name);
        let snapshot = server_entity_snapshot_from(entity);
        inner
            .entity_type_aabbs
            .entry(snapshot.type_id)
            .or_insert(aabb);
        track_entity_chunk_locked(&mut inner, snapshot.id, snapshot.position);
        initialize_entity_wire_state_from_snapshot_locked(&mut inner, &snapshot);
        publications.push(snapshot);
    }
    Some(install_committed_entity_publications_locked(
        &mut inner,
        publications,
    ))
}

fn commit_villager_golem_memory(registry: &SessionRegistry, villagers: Vec<EntityId>, tick: u64) {
    if villagers.is_empty() {
        return;
    }
    let mut villagers = villagers;
    villagers.sort_unstable();
    villagers.dedup();
    let mut entities = registry.lock_entities("commit villager golem memory");
    let mut transitions = Vec::new();
    for entity_id in villagers {
        let Some(current) = entities.snapshot(entity_id) else {
            continue;
        };
        if !is_adult_villager_snapshot(&current) {
            continue;
        }
        let Some(mut brain) = current.retained.villager_brain.clone() else {
            continue;
        };
        if brain.golem_detected_recently(tick) {
            continue;
        }
        brain.note_golem_detected(tick);
        let mut next = current.clone();
        next.retained.villager_brain = Some(brain);
        transitions.push((current, next));
    }
    if !transitions.is_empty() {
        let _ = entities.replace_snapshots_if_current(transitions);
    }
}

fn plan_golem_combat(
    projections: &[EntitySimulationProjection],
    tick: u64,
) -> (Vec<(EntityId, GoalState)>, Vec<PlannedGolemAttack>) {
    let golems = projections
        .iter()
        .filter(|entity| {
            entity.lifecycle == EntityLifecycle::Alive && entity.type_name == "minecraft:iron_golem"
        })
        .collect::<Vec<_>>();
    let hostiles = projections
        .iter()
        .filter(|entity| {
            entity.lifecycle == EntityLifecycle::Alive
                && is_hostile_entity(&entity.type_name)
                && entity.type_name != "minecraft:creeper"
        })
        .collect::<Vec<_>>();

    let mut goals = Vec::new();
    let mut attacks = Vec::new();
    for golem in golems {
        let follow_range = golem.follow_range.clamp(1.0, 2_048.0);
        let target = hostiles
            .iter()
            .copied()
            .filter_map(|target| {
                let distance = distance_sq(golem.position, target.position);
                (distance <= follow_range * follow_range).then_some((distance, target))
            })
            .min_by(|left, right| left.0.total_cmp(&right.0))
            .map(|(_, target)| target);
        let next_goal = target.map_or(
            GoalState::Wander {
                speed: GOLEM_WANDER_SPEED,
                period_ticks: GOLEM_WANDER_PERIOD_TICKS,
            },
            |target| GoalState::FollowTarget {
                target: target.id,
                speed: GOLEM_PURSUIT_SPEED,
            },
        );
        if golem.goal != next_goal {
            goals.push((golem.id, next_goal));
        }
        let Some(target) = target else {
            continue;
        };
        let phase = u64::from(golem.id.0.unsigned_abs());
        if !tick
            .wrapping_add(phase)
            .is_multiple_of(GOLEM_ATTACK_INTERVAL)
            || !within_golem_attack_range(
                golem.position,
                &golem.type_name,
                target.position,
                &target.type_name,
            )
        {
            continue;
        }
        attacks.push(PlannedGolemAttack {
            golem_id: golem.id,
            target_id: target.id,
            damage: deterministic_golem_damage(golem.id, golem.attack_damage, tick),
        });
    }
    (goals, attacks)
}

fn commit_golem_attack(
    registry: &SessionRegistry,
    attack: PlannedGolemAttack,
) -> Option<Vec<VisibilityDispatch>> {
    let mut inner = registry.lock_session_entities("commit iron golem attack");
    let golem = inner.entities.snapshot(attack.golem_id)?;
    let target = inner.entities.snapshot(attack.target_id)?;
    if golem.lifecycle != EntityLifecycle::Alive
        || golem.type_name != "minecraft:iron_golem"
        || target.lifecycle != EntityLifecycle::Alive
        || !is_hostile_entity(&target.type_name)
        || target.type_name == "minecraft:creeper"
        || !within_golem_attack_range(
            golem.position,
            &golem.type_name,
            target.position,
            &target.type_name,
        )
    {
        return None;
    }
    let rewards = entity_kill_rewards_locked(&inner, &target);
    let mut outcome = attack_server_entity_locked(
        &mut inner,
        attack.target_id,
        attack.damage,
        None,
        &rewards,
        None,
    )?;
    let knockback_dispatches = if let EntityAttackOutcome::Damaged { damage, .. } = &outcome {
        let resistance = target
            .attributes
            .base(&AttributeKind::Custom(
                "minecraft:knockback_resistance".to_owned(),
            ))
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        let mut velocity = damage.snapshot.velocity;
        velocity.y += GOLEM_VERTICAL_KNOCKBACK * (1.0 - resistance);
        apply_entity_velocity_locked(&mut inner, attack.target_id, velocity)
    } else {
        Vec::new()
    };

    let mut dispatches = outcome.dispatches_mut().drain(..).collect::<Vec<_>>();
    dispatches.extend(knockback_dispatches);
    let events = entity_event_dispatches_locked(&inner, attack.golem_id, GOLEM_ATTACK_EVENT);
    record_entity_dispatches_locked(&mut inner, &events);
    dispatches.extend(events);
    Some(dispatches)
}

fn villagers_detecting_nearby_golems(projections: &[EntitySimulationProjection]) -> Vec<EntityId> {
    let golems = projections
        .iter()
        .filter(|entity| {
            entity.lifecycle == EntityLifecycle::Alive && entity.type_name == "minecraft:iron_golem"
        })
        .collect::<Vec<_>>();
    projections
        .iter()
        .filter(|villager| is_adult_villager(villager))
        .filter(|villager| {
            golems.iter().any(|golem| {
                distance_sq(villager.position, golem.position)
                    <= GOLEM_DETECTION_RANGE * GOLEM_DETECTION_RANGE
            })
        })
        .map(|villager| villager.id)
        .collect()
}

fn is_adult_villager(entity: &EntitySimulationProjection) -> bool {
    entity.lifecycle == EntityLifecycle::Alive
        && entity.type_name == "minecraft:villager"
        && entity.villager.is_some()
        && entity.villager_schedule == Some(mc_entity::villager_26_1_2::VillagerScheduleKind::Adult)
}

fn is_adult_villager_snapshot(entity: &EntitySnapshot) -> bool {
    entity.lifecycle == EntityLifecycle::Alive
        && entity.type_name == "minecraft:villager"
        && entity.retained.villager.is_some()
        && entity
            .retained
            .villager_population
            .as_ref()
            .is_none_or(|population| population.age_ticks >= 0)
        && entity
            .retained
            .villager_brain
            .as_ref()
            .is_some_and(|brain| {
                brain.schedule == mc_entity::villager_26_1_2::VillagerScheduleKind::Adult
            })
}

fn villager_wants_golem(villager: &EntitySimulationProjection, tick: u64) -> bool {
    villager
        .villager_last_slept_tick
        .is_some_and(|last| tick.saturating_sub(last) < 24_000)
        && villager
            .villager_golem_detected_until_tick
            .is_none_or(|expires| tick > expires)
}

fn villagers_within_agreement_box(
    left: &EntitySimulationProjection,
    right: &EntitySimulationProjection,
) -> bool {
    (left.position.x - right.position.x).abs() <= VILLAGER_AGREEMENT_RANGE
        && (left.position.y - right.position.y).abs() <= VILLAGER_AGREEMENT_RANGE
        && (left.position.z - right.position.z).abs() <= VILLAGER_AGREEMENT_RANGE
}

fn villager_has_nearby_threat(
    villager: &EntitySimulationProjection,
    threats: &[&EntitySimulationProjection],
) -> bool {
    threats.iter().any(|threat| {
        villager_threat_distance(&threat.type_name)
            .is_some_and(|range| distance_sq(villager.position, threat.position) <= range * range)
    })
}

fn villager_threat_distance(entity_type: &str) -> Option<f64> {
    Some(match entity_type {
        "minecraft:drowned"
        | "minecraft:husk"
        | "minecraft:vex"
        | "minecraft:zombie"
        | "minecraft:zombie_villager" => 8.0,
        "minecraft:vindicator" | "minecraft:zoglin" => 10.0,
        "minecraft:evoker" | "minecraft:illusioner" | "minecraft:ravager" => 12.0,
        "minecraft:pillager" => 15.0,
        _ => return None,
    })
}

fn find_golem_spawn_position(
    origin: Vec3,
    initiator: EntityId,
    tick: u64,
    world_read: &WorldReadView,
    materials: &BlockMaterialIds,
    projections: &[EntitySimulationProjection],
) -> Option<Vec3> {
    let base = BlockPos {
        x: origin.x.floor() as i32,
        y: origin.y.floor() as i32,
        z: origin.z.floor() as i32,
    };
    for attempt in 0..GOLEM_SPAWN_ATTEMPTS {
        let (dx, dy, dz) = golem_spawn_offset(initiator, tick, attempt);
        let feet = BlockPos {
            x: base.x.saturating_add(dx),
            y: base.y.saturating_add(dy),
            z: base.z.saturating_add(dz),
        };
        let position = Vec3::new(
            f64::from(feet.x) + 0.5,
            f64::from(feet.y),
            f64::from(feet.z) + 0.5,
        );
        if golem_spawn_position_clear(position, world_read, materials)
            && golem_spawn_position_clear_of_entities(position, projections)
        {
            return Some(position);
        }
    }
    None
}

fn golem_spawn_offset(entity: EntityId, tick: u64, attempt: usize) -> (i32, i32, i32) {
    if attempt == 0 {
        return (0, 0, 0);
    }
    let mut value = u64::from(entity.0.unsigned_abs()).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ tick.rotate_left(17)
        ^ (attempt as u64).wrapping_mul(0xD1B5_4A32_D192_ED03);
    let mut next = |range: i32| {
        value ^= value >> 30;
        value = value.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        value ^= value >> 27;
        let width = u64::try_from(range.saturating_mul(2).saturating_add(1)).unwrap_or(1);
        i32::try_from(value % width).unwrap_or_default() - range
    };
    (
        next(GOLEM_SPAWN_HORIZONTAL_RANGE),
        next(GOLEM_SPAWN_VERTICAL_RANGE),
        next(GOLEM_SPAWN_HORIZONTAL_RANGE),
    )
}

fn golem_spawn_position_clear(
    position: Vec3,
    world_read: &WorldReadView,
    materials: &BlockMaterialIds,
) -> bool {
    let aabb = entity_aabb("minecraft:iron_golem");
    let min_x = (position.x - aabb.half_width + f64::EPSILON).floor() as i32;
    let max_x = (position.x + aabb.half_width - f64::EPSILON).floor() as i32;
    let min_z = (position.z - aabb.half_width + f64::EPSILON).floor() as i32;
    let max_z = (position.z + aabb.half_width - f64::EPSILON).floor() as i32;
    let min_y = position.y.floor() as i32;
    let max_y = (position.y + aabb.height - f64::EPSILON).floor() as i32;
    let support = world_read.get_cached_block(BlockPos {
        x: position.x.floor() as i32,
        y: min_y.saturating_sub(1),
        z: position.z.floor() as i32,
    });
    if !support.is_some_and(|state| materials.classify(state.0).is_solid()) {
        return false;
    }
    for x in min_x..=max_x {
        for z in min_z..=max_z {
            for y in min_y..=max_y {
                let Some(state) = world_read.get_cached_block(BlockPos { x, y, z }) else {
                    return false;
                };
                if materials.classify(state.0) != BlockMaterial::Air {
                    return false;
                }
            }
        }
    }
    true
}

fn golem_spawn_position_clear_of_entities(
    position: Vec3,
    projections: &[EntitySimulationProjection],
) -> bool {
    let golem = entity_aabb("minecraft:iron_golem");
    projections
        .iter()
        .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
        .all(|entity| {
            let other = entity_aabb(&entity.type_name);
            !aabbs_intersect(position, golem, entity.position, other)
        })
}

fn aabbs_intersect(
    left_position: Vec3,
    left: mc_physics::Aabb,
    right_position: Vec3,
    right: mc_physics::Aabb,
) -> bool {
    left_position.x - left.half_width < right_position.x + right.half_width
        && right_position.x - right.half_width < left_position.x + left.half_width
        && left_position.y < right_position.y + right.height
        && right_position.y < left_position.y + left.height
        && left_position.z - left.half_width < right_position.z + right.half_width
        && right_position.z - right.half_width < left_position.z + left.half_width
}

fn within_golem_attack_range(
    golem_position: Vec3,
    golem_type: &str,
    target_position: Vec3,
    target_type: &str,
) -> bool {
    let golem_box = entity_aabb(golem_type);
    let target_box = entity_aabb(target_type);
    let left_min_x = golem_position.x - golem_box.half_width - DEFAULT_ATTACK_REACH;
    let left_max_x = golem_position.x + golem_box.half_width + DEFAULT_ATTACK_REACH;
    let left_min_y = golem_position.y;
    let left_max_y = golem_position.y + golem_box.height;
    let left_min_z = golem_position.z - golem_box.half_width - DEFAULT_ATTACK_REACH;
    let left_max_z = golem_position.z + golem_box.half_width + DEFAULT_ATTACK_REACH;
    let right_min_x = target_position.x - target_box.half_width;
    let right_max_x = target_position.x + target_box.half_width;
    let right_min_y = target_position.y;
    let right_max_y = target_position.y + target_box.height;
    let right_min_z = target_position.z - target_box.half_width;
    let right_max_z = target_position.z + target_box.half_width;
    left_min_x < right_max_x
        && right_min_x < left_max_x
        && left_min_y < right_max_y
        && right_min_y < left_max_y
        && left_min_z < right_max_z
        && right_min_z < left_max_z
}

fn deterministic_golem_damage(golem: EntityId, attack_damage: f64, tick: u64) -> f32 {
    let base = if attack_damage.is_finite() && attack_damage > 0.0 {
        attack_damage as f32
    } else {
        15.0
    };
    let bound = base.floor().max(0.0) as u64;
    if bound == 0 {
        return base;
    }
    let mixed = u64::from(golem.0.unsigned_abs()).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ tick.rotate_left(11);
    base / 2.0 + (mixed % bound) as f32
}

fn deterministic_golem_uuid(entity: EntityId, tick: u64, position: Vec3) -> uuid::Uuid {
    let high = u64::from(entity.0.unsigned_abs()).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ tick.rotate_left(13);
    let low = position.x.to_bits()
        ^ position.y.to_bits().rotate_left(21)
        ^ position.z.to_bits().rotate_left(42);
    uuid::Uuid::from_u128((u128::from(high) << 64) | u128::from(low))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashSet};
    use std::sync::Arc;

    use mc_data::Identifier;
    use mc_entity::villager_26_1_2::{VillagerBrainState, VillagerPoiSet, VillagerScheduleKind};
    use mc_entity::villager_population_26_1_2::VillagerPopulationState;
    use mc_entity::{VillagerData, VillagerKind, VillagerProfession};
    use mc_world::{BlockRegistry, BlockStateId, Chunk, ChunkPos};
    use tokio::sync::mpsc;

    use crate::play::persistence::PersistedEntityCheckpoint;
    use crate::play::{LoggedInProfile, PlayerPose};

    use super::*;

    fn defense_world() -> (WorldReadView, BlockMaterialIds) {
        let block = |name: &str, id: u32| mc_data::blocks::BlockReport {
            id: Identifier::parse(name).unwrap(),
            properties: BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id,
                default: true,
                properties: BTreeMap::new(),
            }],
        };
        let blocks = Arc::new(
            BlockRegistry::from_report(&[block("minecraft:air", 0), block("minecraft:stone", 1)])
                .unwrap(),
        );
        let mut world = mc_world::WorldStorage::in_memory(Arc::clone(&blocks));
        let chunk_pos = ChunkPos { x: 0, z: 0 };
        let mut chunk = Chunk::empty(
            chunk_pos,
            BlockStateId(0),
            Identifier::parse("minecraft:plains").unwrap(),
        );
        for x in 0..16 {
            for z in 0..16 {
                let _ = chunk.set_block(x, 63, z, BlockStateId(1));
            }
        }
        world.insert_generated_chunk(chunk_pos, chunk).unwrap();
        (world.read_view(), BlockMaterialIds::new(0, None, None))
    }

    fn register_observer(registry: &SessionRegistry) -> (u64, mpsc::Receiver<OutboundCommand>) {
        let profile = LoggedInProfile {
            uuid: crate::login::offline_uuid("GolemObserver"),
            name: "GolemObserver".to_owned(),
        };
        let (tx, rx) = mpsc::channel(64);
        let session = registry
            .register(
                &profile,
                (0, 0),
                2,
                HashSet::new(),
                tx,
                PlayerPose::new(0.5, 64.0, 0.5),
            )
            .0;
        assert!(registry.mark_loaded(session, (0, 0)).is_empty());
        (session, rx)
    }

    fn spawn_villager(registry: &SessionRegistry, position: Vec3, tick: u64) -> EntityId {
        let mut entity = SpawnEntity::new(139, "minecraft:villager", position);
        entity.retained.villager = Some(VillagerData::new(
            VillagerKind::Plains,
            VillagerProfession::None,
            1,
        ));
        entity.retained.villager_population = Some(VillagerPopulationState::adult());
        let mut brain = VillagerBrainState::adult(VillagerPoiSet {
            home: Some(position),
            job_site: None,
            meeting_point: Some(position),
        });
        brain.schedule = VillagerScheduleKind::Adult;
        brain.last_slept_tick = Some(tick);
        entity.retained.villager_brain = Some(brain);
        apply_entity_facts(&mut entity);
        registry
            .lock_entities("spawn village defence villager")
            .spawn(entity)
    }

    fn spawn_mob(
        registry: &SessionRegistry,
        type_id: i32,
        type_name: &str,
        position: Vec3,
    ) -> EntityId {
        spawn_mob_with_max_health(registry, type_id, type_name, position, None)
    }

    fn spawn_mob_with_max_health(
        registry: &SessionRegistry,
        type_id: i32,
        type_name: &str,
        position: Vec3,
        max_health: Option<f64>,
    ) -> EntityId {
        let mut entity = SpawnEntity::new(type_id, type_name, position);
        apply_entity_facts(&mut entity);
        if let Some(max_health) = max_health {
            entity
                .attributes
                .set_base(AttributeKind::MaxHealth, max_health);
        }
        registry
            .lock_entities("spawn village defence mob")
            .spawn(entity)
    }

    #[test]
    fn three_recently_slept_villagers_spawn_one_persisted_golem_and_memory_blocks_duplicate() {
        let registry = SessionRegistry::new();
        let (_observer, _outbound) = register_observer(&registry);
        let (world, materials) = defense_world();
        let villagers = [
            spawn_villager(&registry, Vec3::new(4.5, 64.0, 4.5), 90),
            spawn_villager(&registry, Vec3::new(5.5, 64.0, 4.5), 90),
            spawn_villager(&registry, Vec3::new(4.5, 64.0, 5.5), 90),
        ];
        let zombie = spawn_mob(
            &registry,
            151,
            "minecraft:zombie",
            Vec3::new(7.5, 64.0, 4.5),
        );
        registry.publish_active_simulation_entities_for_test(villagers.into_iter().chain([zombie]));
        registry.synchronize_entity_lifecycle_epoch(100);

        let (report, dispatches) = registry.tick_village_defense(
            &SimulationAuthority::for_test(),
            100,
            70,
            Some(&world),
            Some(&materials),
        );
        assert_eq!(report.spawned_golems, 1);
        assert!(dispatches.iter().any(|dispatch| {
            matches!(
                &dispatch.command,
                OutboundCommand::SpawnEntity(entity)
                    if entity.type_name == "minecraft:iron_golem"
            ) || matches!(
                &dispatch.command,
                OutboundCommand::SpawnEntities(entities)
                    if entities.iter().any(|entity| entity.type_name == "minecraft:iron_golem")
            )
        }));
        let records = registry.persisted_entity_records();
        assert_eq!(
            records
                .iter()
                .filter(|record| record.snapshot.type_name == "minecraft:iron_golem")
                .count(),
            1
        );
        let golem = &records
            .iter()
            .find(|record| record.snapshot.type_name == "minecraft:iron_golem")
            .expect("persisted iron golem")
            .snapshot;
        for entity_id in villagers.iter().copied().chain([zombie]) {
            let other = registry
                .lock_entities("read village defence spawn neighbour")
                .snapshot(entity_id)
                .unwrap();
            assert!(!aabbs_intersect(
                golem.position,
                entity_aabb(&golem.type_name),
                other.position,
                entity_aabb(&other.type_name),
            ));
        }
        for villager in villagers {
            let brain = registry
                .lock_entities("read golem detection memory")
                .snapshot(villager)
                .unwrap()
                .retained
                .villager_brain
                .unwrap();
            assert_eq!(brain.golem_detected_until_tick, Some(699));
        }

        let active = records.into_iter().map(|record| record.snapshot.id);
        registry.publish_active_simulation_entities_for_test(active);
        registry.synchronize_entity_lifecycle_epoch(200);
        let (repeat, _) = registry.tick_village_defense(
            &SimulationAuthority::for_test(),
            200,
            70,
            Some(&world),
            Some(&materials),
        );
        assert_eq!(repeat.spawned_golems, 0);
        assert_eq!(
            registry
                .persisted_entity_records()
                .iter()
                .filter(|record| record.snapshot.type_name == "minecraft:iron_golem")
                .count(),
            1
        );
    }

    #[test]
    fn restored_golem_and_villager_memory_prevent_duplicate_spawn() {
        let source = SessionRegistry::new();
        let (_observer, _outbound) = register_observer(&source);
        let (world, materials) = defense_world();
        let villagers = [
            spawn_villager(&source, Vec3::new(4.5, 64.0, 4.5), 90),
            spawn_villager(&source, Vec3::new(5.5, 64.0, 4.5), 90),
            spawn_villager(&source, Vec3::new(4.5, 64.0, 5.5), 90),
        ];
        let zombie = spawn_mob(&source, 151, "minecraft:zombie", Vec3::new(7.5, 64.0, 4.5));
        source.publish_active_simulation_entities_for_test(villagers.into_iter().chain([zombie]));
        source.synchronize_entity_lifecycle_epoch(100);
        assert_eq!(
            source
                .tick_village_defense(
                    &SimulationAuthority::for_test(),
                    100,
                    70,
                    Some(&world),
                    Some(&materials),
                )
                .0
                .spawned_golems,
            1
        );

        let records = source.persisted_entity_records();
        let restored = SessionRegistry::new();
        let (_observer, _outbound) = register_observer(&restored);
        assert_eq!(
            restored
                .restore_persisted_entities(PersistedEntityCheckpoint::new(100, records.clone())),
            records.len()
        );
        let restored_records = restored.persisted_entity_records();
        assert_eq!(
            restored_records
                .iter()
                .filter(|record| record.snapshot.type_name == "minecraft:iron_golem")
                .count(),
            1
        );
        for record in restored_records
            .iter()
            .filter(|record| record.snapshot.type_name == "minecraft:villager")
        {
            let brain = record
                .snapshot
                .retained
                .villager_brain
                .as_ref()
                .expect("restored villager brain");
            assert_eq!(brain.last_slept_tick, Some(90));
            assert_eq!(brain.golem_detected_until_tick, Some(699));
        }

        restored.publish_active_simulation_entities_for_test(
            restored_records.iter().map(|record| record.snapshot.id),
        );
        restored.synchronize_entity_lifecycle_epoch(800);
        let (restored_world, restored_materials) = defense_world();
        let (report, _) = restored.tick_village_defense(
            &SimulationAuthority::for_test(),
            800,
            70,
            Some(&restored_world),
            Some(&restored_materials),
        );
        assert_eq!(report.spawned_golems, 0);
        assert_eq!(
            restored
                .persisted_entity_records()
                .iter()
                .filter(|record| record.snapshot.type_name == "minecraft:iron_golem")
                .count(),
            1
        );
        for record in restored
            .persisted_entity_records()
            .iter()
            .filter(|record| record.snapshot.type_name == "minecraft:villager")
        {
            assert_eq!(
                record
                    .snapshot
                    .retained
                    .villager_brain
                    .as_ref()
                    .and_then(|brain| brain.golem_detected_until_tick),
                Some(1_399)
            );
        }
    }

    #[test]
    fn golem_goal_update_uses_deferred_owner_path() {
        let registry = SessionRegistry::new();
        let golem = spawn_mob(
            &registry,
            70,
            "minecraft:iron_golem",
            Vec3::new(4.5, 64.0, 4.5),
        );
        let goal = GoalState::Wander {
            speed: GOLEM_WANDER_SPEED,
            period_ticks: GOLEM_WANDER_PERIOD_TICKS,
        };
        let applied = registry
            .lock_entities("set village defence fixture goal")
            .set_goals_deferred_journal([(golem, goal.clone())]);
        assert_eq!(applied, 1);
        assert_eq!(
            registry
                .lock_entities("read village defence fixture goal")
                .snapshot(golem)
                .unwrap()
                .goal,
            goal
        );
    }

    #[test]
    fn surviving_damage_accepts_followup_velocity_update() {
        let registry = SessionRegistry::new();
        let zombie = spawn_mob(
            &registry,
            151,
            "minecraft:zombie",
            Vec3::new(6.0, 64.0, 4.5),
        );
        let mut inner = registry.lock_session_entities("damage village defence fixture");
        assert!(
            super::super::entity_combat::damage_server_entity_locked(
                &mut inner, zombie, 16.5, None,
            )
            .is_some()
        );
        assert!(
            inner
                .entities
                .set_velocity(zombie, Vec3::new(0.0, GOLEM_VERTICAL_KNOCKBACK, 0.0),)
        );
    }

    #[test]
    fn golem_attacks_nearby_ravager_with_event_and_vertical_knockback() {
        let registry = SessionRegistry::new();
        registry.configure_arrow_kill_rewards(
            None,
            None,
            None,
            Arc::new(mc_data::items::ItemRegistry::default()),
            Arc::new(mc_data::item_components::ItemFactsTable::default()),
            Arc::new(mc_data::loot::LootTables::default()),
        );
        let (_observer, _outbound) = register_observer(&registry);
        let golem = spawn_mob(
            &registry,
            70,
            "minecraft:iron_golem",
            Vec3::new(4.5, 64.0, 4.5),
        );
        let ravager = spawn_mob_with_max_health(
            &registry,
            109,
            "minecraft:ravager",
            Vec3::new(6.0, 64.0, 4.5),
            Some(100.0),
        );
        registry.publish_active_simulation_entities_for_test([golem, ravager]);
        {
            let mut inner = registry.lock_session_entities("publish golem combat fixture");
            for entity_id in [golem, ravager] {
                let position = inner.entities.snapshot(entity_id).unwrap().position;
                track_entity_chunk_locked(&mut inner, entity_id, position);
                assert!(!spawn_entity_visibility_locked(&mut inner, entity_id).is_empty());
            }
        }
        let phase = u64::from(golem.0.unsigned_abs()) % GOLEM_ATTACK_INTERVAL;
        let due = if phase == 0 {
            GOLEM_ATTACK_INTERVAL
        } else {
            GOLEM_ATTACK_INTERVAL - phase
        };

        let before = registry
            .lock_entities("read ravager before golem attack")
            .snapshot(ravager)
            .unwrap();
        assert!(before.health > 30.0, "ravager health={}", before.health);
        let (report, dispatches) =
            registry.tick_village_defense(&SimulationAuthority::for_test(), due, 70, None, None);
        assert_eq!(report.golem_attacks, 1);
        let after = registry
            .lock_entities("read ravager after golem attack")
            .snapshot(ravager)
            .unwrap();
        assert!(after.health < before.health);
        assert!(after.velocity.y >= GOLEM_VERTICAL_KNOCKBACK - 1.0e-9);
        assert!(dispatches.iter().any(|dispatch| {
            matches!(
                dispatch.command,
                OutboundCommand::EntityEvent { entity_id, event_id }
                    if entity_id == golem.0 && event_id == GOLEM_ATTACK_EVENT
            )
        }));
    }

    #[test]
    fn golem_never_targets_creeper() {
        let registry = SessionRegistry::new();
        let (_observer, _outbound) = register_observer(&registry);
        let golem = spawn_mob(
            &registry,
            70,
            "minecraft:iron_golem",
            Vec3::new(4.5, 64.0, 4.5),
        );
        let creeper = spawn_mob(
            &registry,
            20,
            "minecraft:creeper",
            Vec3::new(5.5, 64.0, 4.5),
        );
        registry.publish_active_simulation_entities_for_test([golem, creeper]);

        let (report, _) =
            registry.tick_village_defense(&SimulationAuthority::for_test(), 20, 70, None, None);
        assert_eq!(report.golem_attacks, 0);
        let goal = registry
            .lock_entities("read golem creeper exclusion")
            .snapshot(golem)
            .unwrap()
            .goal;
        assert!(matches!(goal, GoalState::Wander { .. }), "goal={goal:?}");
    }
}
