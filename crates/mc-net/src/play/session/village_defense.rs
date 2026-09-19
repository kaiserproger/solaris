use std::collections::HashSet;

use mc_entity::{
    AttributeKind, EntityDamageRequest, EntityId, EntityLifecycle, EntitySimulationProjection,
    EntitySnapshot, GoalState, SpawnEntity, Vec3,
};
use mc_physics::{BlockMaterial, BlockMaterialIds};
use mc_world::{BlockPos, WorldReadView};

use crate::play::simulation::SimulationAuthority;

use super::damage_precommit::{EntityDamagePrecommitResume, begin_entity_damage};
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
use crate::play::simulation::SimulationCommand;
use mc_script::precommit::{Approval, HookActor, HookDecision, HookFailure, HookKind};

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

/// The source-side image an approved iron-golem attack must still match.
#[derive(Debug, Clone)]
pub(in crate::play) struct GolemDamageContinuation {
    pub(in crate::play) golem: EntitySnapshot,
}

fn defer_golem_entity_damage_precommit(
    registry: &SessionRegistry,
    expected: EntitySnapshot,
    request: EntityDamageRequest,
    continuation: GolemDamageContinuation,
) -> bool {
    let Some(handle) = registry.damage_precommit_handle().cloned() else {
        return true;
    };
    let Ok(source) = u64::try_from(continuation.golem.id.0).map(HookActor::Entity) else {
        return true;
    };
    let pending = match begin_entity_damage(
        registry,
        expected.id,
        source,
        "mob-attack",
        expected.position,
        request.amount,
    ) {
        Ok(Some(pending)) => pending,
        Ok(None) => return false,
        Err(_) => return true,
    };
    handle.spawn_precommit_resume(pending, None, None, move |decision| {
        SimulationCommand::ResumeEntityDamagePrecommit(Box::new(EntityDamagePrecommitResume {
            expected,
            request,
            attacker_costs: None,
            completion: super::damage_precommit::EntityDamagePrecommitCompletion::Golem(
                continuation,
            ),
            decision,
        }))
    });
    true
}

/// Re-enters the complete native golem attack kernel only for the exact source
/// and target images that asked the guest boundary.
pub(in crate::play) fn resume_golem_entity_damage_precommit(
    registry: &SessionRegistry,
    expected: EntitySnapshot,
    mut request: EntityDamageRequest,
    decision: Result<Approval, HookFailure>,
    continuation: GolemDamageContinuation,
) -> Vec<VisibilityDispatch> {
    let Ok(mut approval) = decision else {
        return Vec::new();
    };
    request.amount = match approval.decision() {
        HookDecision::Keep => request.amount,
        HookDecision::Replace(amount) if amount.is_finite() && amount > 0.0 => amount,
        HookDecision::Cancel | HookDecision::Replace(_) | _ => return Vec::new(),
    };
    let mut inner = registry.lock_session_entities("resume iron golem damage precommit");
    let Some(golem) = inner.entities.snapshot(continuation.golem.id) else {
        return Vec::new();
    };
    if golem != continuation.golem
        || inner.entities.snapshot(expected.id).as_ref() != Some(&expected)
        || golem.lifecycle != EntityLifecycle::Alive
        || golem.type_name != "minecraft:iron_golem"
        || expected.lifecycle != EntityLifecycle::Alive
        || !is_hostile_entity(&expected.type_name)
        || expected.type_name == "minecraft:creeper"
        || !within_golem_attack_range(
            golem.position,
            &golem.type_name,
            expected.position,
            &expected.type_name,
        )
        || approval.consume().is_err()
    {
        return Vec::new();
    }
    let rewards = entity_kill_rewards_locked(&inner, &expected);
    let Some(mut outcome) = attack_server_entity_locked(
        &mut inner,
        expected.id,
        request.amount,
        None,
        &rewards,
        None,
    ) else {
        return Vec::new();
    };
    let knockback_dispatches = if let EntityAttackOutcome::Damaged { damage, .. } = &outcome {
        let resistance = expected
            .attributes
            .base(&AttributeKind::Custom(
                "minecraft:knockback_resistance".to_owned(),
            ))
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        let mut velocity = damage.snapshot.velocity;
        velocity.y += GOLEM_VERTICAL_KNOCKBACK * (1.0 - resistance);
        apply_entity_velocity_locked(&mut inner, expected.id, velocity)
    } else {
        Vec::new()
    };
    let mut dispatches = outcome.dispatches_mut().drain(..).collect::<Vec<_>>();
    dispatches.extend(knockback_dispatches);
    let events = entity_event_dispatches_locked(&inner, golem.id, GOLEM_ATTACK_EVENT);
    record_entity_dispatches_locked(&mut inner, &events);
    dispatches.extend(events);
    dispatches
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
        let active_chunks = self.active_simulation_chunks.load_full();
        if active_chunks.is_empty() {
            return (VillageDefenseReport::default(), Vec::new());
        }
        let periodic_village_work = tick.is_multiple_of(VILLAGE_DEFENSE_TICK_INTERVAL);
        let projection_ids = {
            let inner = self.lock_inner("select active village defence actors");
            let is_active = |entity_id: EntityId| {
                self.simulation_inputs
                    .entity_chunk(entity_id)
                    .is_some_and(|chunk| active_chunks.contains(&chunk))
            };
            let active_golems = inner
                .iron_golem_entities
                .iter()
                .copied()
                .filter(|&entity| is_active(entity))
                .collect::<Vec<_>>();
            let active_hostiles = inner
                .hostile_entities
                .iter()
                .copied()
                .filter(|&entity| is_active(entity))
                .collect::<Vec<_>>();
            if active_golems.is_empty() && active_hostiles.is_empty() {
                return (VillageDefenseReport::default(), Vec::new());
            }
            let mut ids = active_golems.into_iter().collect::<HashSet<_>>();
            ids.extend(active_hostiles);
            if periodic_village_work {
                ids.extend(
                    inner
                        .villager_entities
                        .iter()
                        .copied()
                        .filter(|&entity| is_active(entity)),
                );
            }
            ids
        };
        let projections = self
            .lock_entities("project village defence candidates")
            .simulation_projections_for_ids(&projection_ids);
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
            let mut spawns = plan_golem_spawns(
                &projections,
                tick,
                world_read,
                materials,
                MAX_GOLEM_SPAWNS_PER_TICK,
            );
            if !spawns.is_empty() {
                let collision_projection_ids = self
                    .simulation_inputs
                    .entity_candidates_in_chunks(&active_chunks);
                let collision_projections = self
                    .lock_entities("project village golem spawn collisions")
                    .simulation_projections_for_ids(&collision_projection_ids);
                spawns = plan_golem_spawns(
                    &collision_projections,
                    tick,
                    world_read,
                    materials,
                    MAX_GOLEM_SPAWNS_PER_TICK,
                );
            }
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
            entity.lifecycle == EntityLifecycle::Alive
                && &*entity.type_name == "minecraft:iron_golem"
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
            entity.lifecycle == EntityLifecycle::Alive
                && &*entity.type_name == "minecraft:iron_golem"
        })
        .collect::<Vec<_>>();
    let hostiles = projections
        .iter()
        .filter(|entity| {
            entity.lifecycle == EntityLifecycle::Alive
                && is_hostile_entity(&entity.type_name)
                && &*entity.type_name != "minecraft:creeper"
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
    if registry
        .precommit_boundary()
        .is_some_and(|boundary| boundary.has_precommit_hooks(HookKind::Damage))
    {
        let request = EntityDamageRequest {
            amount: attack.damage,
            tick: inner.entity_lifecycle_tick,
            death_remove_tick: inner
                .entity_lifecycle_tick
                .saturating_add(super::ENTITY_DEATH_TICKS),
            villager_gossip_event: None,
        };
        drop(inner);
        if defer_golem_entity_damage_precommit(
            registry,
            target,
            request,
            GolemDamageContinuation { golem },
        ) {
            return Some(Vec::new());
        }
        unreachable!("a registered damage hook must either defer or refuse golem damage");
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
            entity.lifecycle == EntityLifecycle::Alive
                && &*entity.type_name == "minecraft:iron_golem"
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
        && &*entity.type_name == "minecraft:villager"
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
#[path = "village_defense_tests.rs"]
mod tests;
