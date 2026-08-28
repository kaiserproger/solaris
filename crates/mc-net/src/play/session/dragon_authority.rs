use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use mc_entity::dragon_26_1_2::{
    DragonAirPhase, DragonAirState, DragonPart, charge_recovery_step, choose_d1_attack, death_step,
    dragon_facing_angle_degrees, part_center, steer_flight, strafe_step,
};
use mc_entity::{EntityId, EntityLifecycle, GoalState, Rotation, SpawnEntity, Vec3};

use crate::play::combat::{PlayerDamageKind, PlayerDamageRequest};
use crate::play::simulation::SimulationAuthority;

use super::entity_lifecycle::{remove_server_entity_locked, track_entity_chunk_locked};
use super::outbound::{OutboundCommand, VisibilityDispatch};
use super::pickups::spawn_xp_orb_locked;
use super::projectiles::{initial_hurting_projectile_state, projectile_identity};
use super::visibility::{
    initialize_entity_wire_state_locked, session_recipients, spawn_entity_visibility_locked,
};
use super::{SessionRegistry, apply_entity_facts};

const DRAGON_CLOUD_HEIGHT: f64 = 0.5;
const PLAYER_HEIGHT: f64 = 1.8;
const DRAGON_CLOUD_PULSE_TICKS: u32 = 5;
const DRAGON_CLOUD_DAMAGE: f32 = 6.0;
const DRAGON_HEAD_TARGET_Y_OFFSET: f64 = 0.9;
const DRAGON_WING_DAMAGE_PERIOD_TICKS: u64 = 3;

#[derive(Debug, Clone, Copy)]
struct DragonCloudTarget {
    session_id: u64,
    entity_id: i32,
    position: Vec3,
}

#[derive(Debug, Clone)]
struct DragonAirTarget {
    session_id: u64,
    entity_id: i32,
    position: Vec3,
    visible_entities: Arc<HashSet<EntityId>>,
}

#[derive(Debug, Clone, Copy)]
struct PlannedDragonFireball {
    owner_id: EntityId,
    position: Vec3,
    direction: Vec3,
    rotation: Rotation,
}

fn distance_sq(left: Vec3, right: Vec3) -> f64 {
    let dx = left.x - right.x;
    let dy = left.y - right.y;
    let dz = left.z - right.z;
    dx * dx + dy * dy + dz * dz
}

fn dragon_target<'a>(
    state: &DragonAirState,
    targets: &'a [DragonAirTarget],
    dragon_id: EntityId,
) -> Option<&'a DragonAirTarget> {
    let target =
        state
            .target_session
            .zip(state.target_entity_id)
            .and_then(|(session_id, entity_id)| {
                targets
                    .iter()
                    .find(|target| target.session_id == session_id && target.entity_id == entity_id)
            })?;
    target
        .visible_entities
        .contains(&dragon_id)
        .then_some(target)
}

fn nearest_dragon_target(
    targets: &[DragonAirTarget],
    dragon_id: EntityId,
    position: Vec3,
) -> Option<&DragonAirTarget> {
    targets
        .iter()
        .filter(|target| target.visible_entities.contains(&dragon_id))
        .min_by(|left, right| {
            distance_sq(position, left.position).total_cmp(&distance_sq(position, right.position))
        })
}

fn planned_dragon_fireball(
    dragon_id: EntityId,
    state: &DragonAirState,
    dragon_position: Vec3,
    rotation: Rotation,
    target: Vec3,
) -> Option<PlannedDragonFireball> {
    let head = part_center(state, dragon_position, rotation.yaw, DragonPart::Head)?;
    let yaw = f64::from(rotation.yaw).to_radians();
    let view_x = yaw.sin();
    let view_z = -yaw.cos();
    let position = Vec3::new(head.x - view_x, head.y + 1.0, head.z - view_z);
    let direction = Vec3::new(
        target.x - position.x,
        target.y + DRAGON_HEAD_TARGET_Y_OFFSET - position.y,
        target.z - position.z,
    );
    direction.is_finite().then_some(PlannedDragonFireball {
        owner_id: dragon_id,
        position,
        direction,
        rotation,
    })
}

fn target_overlaps_dragon_part(target: Vec3, center: Vec3, part: DragonPart, inflate: f64) -> bool {
    let dimensions = part.dimensions();
    let horizontal = dimensions.width * 0.5 + inflate + 0.3;
    let min_y = center.y - dimensions.height * 0.5 - inflate;
    let max_y = center.y + dimensions.height * 0.5 + inflate;
    (target.x - center.x).abs() <= horizontal
        && (target.z - center.z).abs() <= horizontal
        && target.y <= max_y
        && target.y + PLAYER_HEIGHT >= min_y
}

fn dragon_contact_damage(
    state: &DragonAirState,
    position: Vec3,
    rotation: Rotation,
    target: Vec3,
    tick: u64,
) -> Option<f32> {
    for part in [DragonPart::Head, DragonPart::Neck] {
        let center = part_center(state, position, rotation.yaw, part)?;
        if target_overlaps_dragon_part(target, center, part, 1.0) {
            return Some(mc_entity::dragon_26_1_2::HEAD_NECK_CONTACT_DAMAGE);
        }
    }
    if !tick.is_multiple_of(DRAGON_WING_DAMAGE_PERIOD_TICKS) {
        return None;
    }
    for part in [DragonPart::Wing1, DragonPart::Wing2] {
        let center = part_center(state, position, rotation.yaw, part)?;
        if target_overlaps_dragon_part(target, center, part, 4.0) {
            return Some(mc_entity::dragon_26_1_2::WING_CONTACT_DAMAGE);
        }
    }
    None
}

impl SessionRegistry {
    pub(in crate::play) fn tick_dragon_air_combat(
        &self,
        _authority: &SimulationAuthority,
        tick: u64,
    ) -> Vec<VisibilityDispatch> {
        let active_ids = self.active_simulation_entities.load_full();
        if active_ids.is_empty() {
            return Vec::new();
        }

        let mut dragon_ids = {
            let entities = self.lock_entities("scan active ender dragons");
            entities.prefetch(&active_ids);
            active_ids
                .iter()
                .copied()
                .filter(|&id| {
                    entities.snapshot(id).is_some_and(|snapshot| {
                        snapshot.lifecycle == EntityLifecycle::Alive
                            && snapshot.type_name == "minecraft:ender_dragon"
                    })
                })
                .collect::<Vec<_>>()
        };
        if dragon_ids.is_empty() {
            return Vec::new();
        }
        dragon_ids.sort_unstable();

        let targets = self
            .movement_recipients
            .load_full()
            .values()
            .filter_map(|publication| {
                let (target, visible_entities) = publication.combat_target_snapshot()?;
                target.is_targetable().then_some(DragonAirTarget {
                    session_id: publication.id(),
                    entity_id: publication.entity_id(),
                    position: Vec3::new(target.pose().x, target.pose().y, target.pose().z),
                    visible_entities,
                })
            })
            .collect::<Vec<_>>();
        let fireball_type_id = self
            .hostile_dragon_fireball_entity_type_id
            .load(Ordering::Acquire);
        let fireball_type_id = (fireball_type_id >= 0).then_some(fireball_type_id);
        let dragon_id_set = dragon_ids.iter().copied().collect::<HashSet<_>>();
        let mut inner = self.lock_session_entities("tick ender dragon air combat");
        inner.entities.prefetch(&dragon_id_set);
        let mut dispatches = Vec::new();

        for dragon_id in dragon_ids {
            let Some(expected) = inner.entities.snapshot(dragon_id) else {
                continue;
            };
            if expected.lifecycle != EntityLifecycle::Alive
                || expected.type_name != "minecraft:ender_dragon"
            {
                continue;
            }
            let mut state = expected
                .retained
                .dragon_air
                .unwrap_or_else(|| DragonAirState::new(expected.position, expected.rotation.yaw));
            if state.phase == DragonAirPhase::Dying {
                let step = death_step(state.death_time);
                state.death_time = step.next_death_time;
                state.record_flight_sample(expected.position.y, expected.rotation.yaw);
                let mut next = expected.clone();
                next.position = Vec3::new(
                    expected.position.x,
                    expected.position.y + 0.1,
                    expected.position.z,
                );
                next.velocity = Vec3::new(0.0, 0.1, 0.0);
                next.on_ground = false;
                next.goal = GoalState::Idle;
                next.health = 1.0;
                next.retained.dragon_air = Some(state);
                let death_position = next.position;
                if !inner.entities.replace_snapshot_if_current(expected, next) {
                    continue;
                }
                if step.xp_award > 0
                    && let Some(xp_type_id) = inner.arrow_kill_rewards.xp_orb_entity_type_id
                {
                    dispatches.extend(spawn_xp_orb_locked(
                        &mut inner,
                        xp_type_id,
                        death_position,
                        i32::try_from(step.xp_award).unwrap_or(i32::MAX),
                    ));
                }
                if step.remove
                    && let Some((_, removed)) = remove_server_entity_locked(&mut inner, dragon_id)
                {
                    dispatches.extend(removed);
                }
                continue;
            }
            state.record_flight_sample(expected.position.y, expected.rotation.yaw);
            let mut fireball = None;

            match state.phase {
                DragonAirPhase::HoldingPattern => {
                    if let Some(target) =
                        nearest_dragon_target(&targets, dragon_id, expected.position)
                    {
                        let phase = choose_d1_attack(
                            state.attack_cycle,
                            distance_sq(expected.position, target.position),
                        );
                        match phase {
                            DragonAirPhase::StrafePlayer => state.begin_strafe(
                                target.session_id,
                                target.entity_id,
                                target.position,
                            ),
                            DragonAirPhase::ChargingPlayer => state.begin_charge(
                                target.session_id,
                                target.entity_id,
                                target.position,
                            ),
                            _ => {}
                        }
                    } else if state.fly_target.is_some_and(|target| {
                        let distance = distance_sq(expected.position, target);
                        !(100.0..=22_500.0).contains(&distance)
                    }) {
                        state.return_to_holding();
                    }
                }
                DragonAirPhase::StrafePlayer => {
                    let Some(target) = dragon_target(&state, &targets, dragon_id) else {
                        state.return_to_holding();
                        continue;
                    };
                    let dx = target.position.x - expected.position.x;
                    let dz = target.position.z - expected.position.z;
                    let horizontal = dx.hypot(dz);
                    let height_offset = (0.4 + horizontal / 80.0 - 1.0).min(10.0);
                    state.fly_target = Some(Vec3::new(
                        target.position.x,
                        target.position.y + height_offset,
                        target.position.z,
                    ));
                    let angle = dragon_facing_angle_degrees(
                        expected.rotation.yaw,
                        expected.position,
                        target.position,
                    )
                    .unwrap_or(180.0);
                    let step = strafe_step(
                        state.fireball_charge,
                        distance_sq(expected.position, target.position),
                        true,
                        angle,
                    );
                    state.fireball_charge = step.next_charge;
                    if step.fire {
                        fireball = planned_dragon_fireball(
                            dragon_id,
                            &state,
                            expected.position,
                            expected.rotation,
                            target.position,
                        );
                        state.return_to_holding();
                    }
                }
                DragonAirPhase::ChargingPlayer => {
                    if dragon_target(&state, &targets, dragon_id).is_none() {
                        state.return_to_holding();
                        continue;
                    }
                    let arrived = state.fly_target.is_some_and(|target| {
                        let distance = distance_sq(expected.position, target);
                        !(100.0..=22_500.0).contains(&distance)
                    });
                    let (recovery, done) =
                        charge_recovery_step(state.charge_recovery_ticks, arrived);
                    state.charge_recovery_ticks = recovery;
                    if done {
                        state.return_to_holding();
                    }
                }
                DragonAirPhase::Dying => continue,
            }

            let Some(fly_target) = state.fly_target else {
                continue;
            };
            let Some(motion) = steer_flight(
                state.phase,
                expected.position,
                expected.velocity,
                expected.rotation.yaw,
                state.yaw_accel,
                fly_target,
            ) else {
                continue;
            };
            state.yaw_accel = motion.yaw_accel;
            let mut next = expected.clone();
            next.position = motion.position;
            next.velocity = motion.velocity;
            next.rotation = Rotation {
                yaw: motion.yaw,
                pitch: expected.rotation.pitch,
                head_yaw: motion.yaw,
            };
            next.on_ground = false;
            next.goal = GoalState::Idle;
            next.retained.dragon_air = Some(state);
            let contact_position = next.position;
            let contact_rotation = next.rotation;
            if !inner.entities.replace_snapshot_if_current(expected, next) {
                continue;
            }

            if let (Some(fireball_type_id), Some(shot)) = (fireball_type_id, fireball) {
                let Some(projectile_state) = initial_hurting_projectile_state(
                    Some(projectile_identity(shot.owner_id)),
                    "minecraft:dragon_fireball",
                    shot.position,
                    shot.direction,
                    shot.rotation,
                ) else {
                    continue;
                };
                let mut projectile =
                    SpawnEntity::new(fireball_type_id, "minecraft:dragon_fireball", shot.position);
                projectile.retained.spawn_tick = inner.entity_lifecycle_tick;
                projectile.velocity = Vec3::new(
                    projectile_state.projectile.velocity.x,
                    projectile_state.projectile.velocity.y,
                    projectile_state.projectile.velocity.z,
                );
                projectile.rotation = shot.rotation;
                projectile.on_ground = false;
                apply_entity_facts(&mut projectile);
                projectile.retained.hurting_projectile_state = Some(projectile_state);
                let projectile_id = inner.entities.spawn(projectile);
                inner
                    .entity_type_aabbs
                    .entry(fireball_type_id)
                    .or_insert_with(|| {
                        super::interaction_geometry::entity_aabb("minecraft:dragon_fireball")
                    });
                track_entity_chunk_locked(&mut inner, projectile_id, shot.position);
                initialize_entity_wire_state_locked(&mut inner, projectile_id);
                dispatches.extend(spawn_entity_visibility_locked(&mut inner, projectile_id));
            }

            for target in &targets {
                let Some(amount) = dragon_contact_damage(
                    &state,
                    contact_position,
                    contact_rotation,
                    target.position,
                    tick,
                ) else {
                    continue;
                };
                for recipient in session_recipients(&inner, [target.session_id]) {
                    dispatches.push(VisibilityDispatch {
                        recipient,
                        command: OutboundCommand::DamagePlayer {
                            damage: PlayerDamageRequest {
                                kind: PlayerDamageKind::MobAttack,
                                amount,
                                source_origin: Some(contact_position),
                            },
                        },
                    });
                }
            }
        }

        dispatches
    }

    pub(in crate::play) fn tick_dragon_breath_clouds(
        &self,
        _authority: &SimulationAuthority,
        tick: u64,
    ) -> Vec<VisibilityDispatch> {
        let active_ids = self.active_simulation_entities.load_full();
        if active_ids.is_empty() {
            return Vec::new();
        }

        // Preserve the ordinary hostile lock contract: do not acquire the combined
        // session/entity guard unless an actual dragon cloud is active.
        let mut cloud_ids = {
            let entities = self.lock_entities("scan active dragon breath clouds");
            entities.prefetch(&active_ids);
            active_ids
                .iter()
                .copied()
                .filter(|&id| {
                    entities.snapshot(id).is_some_and(|snapshot| {
                        snapshot.lifecycle == EntityLifecycle::Alive
                            && snapshot.type_name == "minecraft:area_effect_cloud"
                            && snapshot.retained.dragon_breath_cloud.is_some()
                    })
                })
                .collect::<Vec<_>>()
        };
        if cloud_ids.is_empty() {
            return Vec::new();
        }
        cloud_ids.sort_unstable();

        let targets = self
            .movement_recipients
            .load_full()
            .values()
            .filter_map(|publication| {
                let (target, _) = publication.combat_target_snapshot()?;
                target.is_targetable().then_some(DragonCloudTarget {
                    session_id: publication.id(),
                    entity_id: publication.entity_id(),
                    position: Vec3::new(target.pose().x, target.pose().y, target.pose().z),
                })
            })
            .collect::<Vec<_>>();

        let cloud_id_set = cloud_ids.iter().copied().collect::<HashSet<_>>();
        let mut inner = self.lock_session_entities("tick dragon breath clouds");
        inner.entities.prefetch(&cloud_id_set);
        let mut dispatches = Vec::new();

        for cloud_id in cloud_ids {
            let Some(expected) = inner.entities.snapshot(cloud_id) else {
                continue;
            };
            if expected.lifecycle != EntityLifecycle::Alive
                || expected.type_name != "minecraft:area_effect_cloud"
            {
                continue;
            }
            let Some(mut cloud) = expected.retained.dragon_breath_cloud.clone() else {
                continue;
            };

            cloud.age_ticks = cloud.age_ticks.saturating_add(1);
            if cloud.age_ticks >= cloud.duration_ticks {
                if let Some((_, removed)) = remove_server_entity_locked(&mut inner, cloud_id) {
                    dispatches.extend(removed);
                }
                continue;
            }

            cloud.radius += cloud.radius_per_tick;
            if !cloud.radius.is_finite() || cloud.radius < 0.5 {
                if let Some((_, removed)) = remove_server_entity_locked(&mut inner, cloud_id) {
                    dispatches.extend(removed);
                }
                continue;
            }

            let pulse = cloud.age_ticks.is_multiple_of(DRAGON_CLOUD_PULSE_TICKS);
            let mut damaged_sessions = Vec::new();
            if pulse {
                cloud.victims.retain(|victim| tick < victim.next_apply_tick);
                let radius_sq = f64::from(cloud.radius) * f64::from(cloud.radius);
                for target in &targets {
                    if target.entity_id == cloud.owner_entity_id
                        || target.position.y > expected.position.y + DRAGON_CLOUD_HEIGHT
                        || target.position.y + PLAYER_HEIGHT < expected.position.y
                    {
                        continue;
                    }
                    let dx = target.position.x - expected.position.x;
                    let dz = target.position.z - expected.position.z;
                    if dx * dx + dz * dz > radius_sq
                        || cloud
                            .victims
                            .iter()
                            .any(|victim| victim.session_id == target.session_id)
                    {
                        continue;
                    }
                    cloud
                        .victims
                        .push(mc_entity::EntityDragonBreathCloudVictim {
                            session_id: target.session_id,
                            next_apply_tick: tick
                                .saturating_add(u64::from(cloud.reapplication_delay_ticks)),
                        });
                    damaged_sessions.push(target.session_id);
                }
            }

            let source_origin = expected.position;
            let mut next = expected.clone();
            next.retained.dragon_breath_cloud = Some(cloud);
            if !inner.entities.replace_snapshot_if_current(expected, next) {
                continue;
            }

            for session_id in damaged_sessions {
                for recipient in session_recipients(&inner, [session_id]) {
                    dispatches.push(VisibilityDispatch {
                        recipient,
                        command: OutboundCommand::DamagePlayer {
                            damage: PlayerDamageRequest {
                                kind: PlayerDamageKind::IndirectMagic,
                                amount: DRAGON_CLOUD_DAMAGE,
                                source_origin: Some(source_origin),
                            },
                        },
                    });
                }
            }
        }

        dispatches
    }
}
