use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use mc_data::mob_behavior_26_1_2::{MobBehaviorTable, MobCombatPolicy};
use mc_entity::{
    AttributeKind, EntityId, EntityLifecycle, EntityPrimedTntState, EntitySnapshot, GoalState,
    Rotation, SpawnEntity, Vec3,
};
use mc_world::BlockStateId;

#[cfg(test)]
use crate::play::HOSTILE_FOLLOW_SPEED;
use crate::play::combat::{PlayerDamageKind, PlayerDamageRequest};
use crate::play::simulation::SimulationAuthority;
use crate::play::{
    CREEPER_CANCEL_RANGE, CREEPER_FUSE_TICKS, CREEPER_TRIGGER_RANGE, HOSTILE_MELEE_PERIOD_TICKS,
    HOSTILE_MELEE_RANGE, HOSTILE_MELEE_VERTICAL_REACH, SKELETON_ARROW_SPEED,
    SKELETON_SHOT_PERIOD_TICKS, SKELETON_SHOT_RANGE,
};

#[cfg(test)]
use super::entity_lifecycle::nearby_entity_snapshots_locked;
use super::entity_lifecycle::{nearby_entity_candidate_ids_locked, track_entity_chunk_locked};
use super::entity_owner::EntityOwnerAccess;
use super::explosion_authority::schedule_primed_tnt_deadline_locked;
use super::interaction_geometry::{distance_sq, entity_aabb};
#[cfg(test)]
use super::outbound::ServerEntitySnapshot;
use super::outbound::{OutboundCommand, VisibilityDispatch};
use super::projectiles::{initial_arrow_state, projectile_identity};
use super::visibility::{
    initialize_entity_wire_state_from_snapshot_locked, server_entity_snapshot_from,
    session_recipients, spawn_entity_visibility_from_snapshot_locked, visibility_dispatches,
    visible_entity_observers_locked,
};
use super::{SessionRegistry, apply_entity_facts, is_hostile_entity};

struct HostileAttackTickEntity {
    id: EntityId,
    kind: HostileAttackKind,
    position: Vec3,
    rotation: Rotation,
}

struct HostileTargetTickSession {
    id: super::SessionId,
    position: Vec3,
    visible_entities: Arc<HashSet<EntityId>>,
}

struct PlannedCreeperFuse {
    hostile_id: EntityId,
    nearest_distance_sq: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
enum HostileAttackKind {
    Creeper,
    Skeleton,
    Melee { attack_damage: f32 },
}

struct PlannedSkeletonAttack {
    hostile_id: EntityId,
    arrow_entity_type_id: i32,
    position: Vec3,
    velocity: Vec3,
    rotation: Rotation,
}

struct PlannedMeleeAttack {
    hostile_id: EntityId,
    target_session: super::SessionId,
    amount: f32,
}

struct SpawnedHostileArrow {
    hostile_id: EntityId,
    snapshot: EntitySnapshot,
}

#[cfg(test)]
#[derive(Debug)]
pub(super) struct HostileScanProbe {
    pub(super) reached: std::sync::mpsc::Sender<()>,
    pub(super) resume: std::sync::mpsc::Receiver<()>,
}

#[cfg(test)]
#[derive(Debug)]
pub(super) struct HostileCommitProbe {
    pub(super) reached: std::sync::mpsc::Sender<()>,
    pub(super) resume: std::sync::mpsc::Receiver<()>,
}

impl SessionRegistry {
    pub(in crate::play::session) fn reconcile_hostile_targets_after_live_session_change(&self) {
        loop {
            let (generation, player_positions) = {
                let inner = self.lock_inner("snapshot live players for hostile reconciliation");
                let generation = self.live_session_generation.load(Ordering::Acquire);
                let player_positions = inner
                    .sessions
                    .iter()
                    .filter(|(session_id, _)| {
                        !inner.dead_sessions.contains(session_id)
                            && !inner.spectator_sessions.contains(session_id)
                            && !inner.client_unloaded_sessions.contains(session_id)
                    })
                    .map(|(_, session)| Vec3::new(session.pose.x, session.pose.y, session.pose.z))
                    .collect::<Vec<_>>();
                (generation, player_positions)
            };
            #[cfg(test)]
            self.pause_before_hostile_reconciliation_for_test();
            let mob_behaviors = self.mob_behavior_table();
            let mut entities = self.lock_entities("reconcile hostiles after live session change");
            update_hostile_targets(&mut entities, &player_positions, None, &mob_behaviors);
            drop(entities);
            if self.live_session_generation.load(Ordering::Acquire) == generation {
                return;
            }
        }
    }

    #[cfg(test)]
    fn pause_before_hostile_reconciliation_for_test(&self) {
        let probe = self
            .hostile_reconcile_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe.reached.send(()).expect("hostile reconcile receiver");
            probe.resume.recv().expect("hostile reconcile release");
        }
    }

    #[cfg(test)]
    fn pause_during_hostile_scan_for_test(&self) {
        let probe = self
            .hostile_scan_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe.reached.send(()).expect("hostile scan receiver");
            probe.resume.recv().expect("hostile scan release");
        }
    }

    #[cfg(test)]
    fn pause_between_hostile_entity_and_session_commit_for_test(&self) {
        let probe = self
            .hostile_commit_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe.reached.send(()).expect("hostile commit receiver");
            probe.resume.recv().expect("hostile commit release");
        }
    }

    #[cfg(test)]
    fn pause_before_hostile_session_publication_for_test(&self) {
        let probe = self
            .hostile_publication_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("hostile publication receiver");
            probe.resume.recv().expect("hostile publication release");
        }
    }

    #[cfg(test)]
    fn pause_after_hostile_target_snapshot_for_test(&self) {
        let probe = self
            .hostile_target_snapshot_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe.reached.send(()).expect("hostile target receiver");
            probe.resume.recv().expect("hostile target release");
        }
    }

    pub(in crate::play) fn tick_hostile_attacks(
        &self,
        _authority: &SimulationAuthority,
        tick: u64,
        air: BlockStateId,
    ) -> (usize, Vec<VisibilityDispatch>) {
        let loaded_entity_ids = self.active_hostile_entities.load_full();
        if loaded_entity_ids.is_empty() {
            return (0, Vec::new());
        }
        let mob_behaviors = self.mob_behavior_table();
        let mut hostiles = Vec::new();
        {
            let entities = self.lock_entities("scan hostile attack candidates");
            #[cfg(test)]
            self.pause_during_hostile_scan_for_test();
            entities.visit_simulation_entities_for_ids(&loaded_entity_ids, |entity| {
                #[cfg(test)]
                self.hostile_entity_scan_visits
                    .fetch_add(1, Ordering::Relaxed);
                if entity.lifecycle != EntityLifecycle::Alive {
                    return;
                }
                let Some(profile) = mob_behaviors.get_by_name(entity.type_name) else {
                    return;
                };
                let kind = match profile.combat {
                    MobCombatPolicy::CreeperFuse => HostileAttackKind::Creeper,
                    MobCombatPolicy::Arrow => HostileAttackKind::Skeleton,
                    MobCombatPolicy::Melee => HostileAttackKind::Melee {
                        attack_damage: entity
                            .attributes
                            .base(&AttributeKind::AttackDamage)
                            .unwrap_or(3.0) as f32,
                    },
                    MobCombatPolicy::None | MobCombatPolicy::UnsupportedSpecial => return,
                };
                let period = match kind {
                    HostileAttackKind::Creeper => 1,
                    HostileAttackKind::Skeleton => SKELETON_SHOT_PERIOD_TICKS,
                    HostileAttackKind::Melee { .. } => HOSTILE_MELEE_PERIOD_TICKS,
                };
                let phase = u64::from(entity.id.0.unsigned_abs());
                if !tick.wrapping_add(phase).is_multiple_of(period) {
                    return;
                }
                hostiles.push(HostileAttackTickEntity {
                    id: entity.id,
                    kind,
                    position: entity.position,
                    rotation: entity.rotation,
                });
            });
        }
        #[cfg(test)]
        self.hostile_attack_candidates
            .fetch_add(hostiles.len() as u64, Ordering::Relaxed);
        if hostiles.is_empty() {
            return (0, Vec::new());
        }

        let targets = self
            .movement_recipients
            .load_full()
            .values()
            .filter_map(|publication| {
                let (target, visible_entities) = publication.combat_target_snapshot()?;
                target.is_targetable().then_some(HostileTargetTickSession {
                    id: publication.id(),
                    position: Vec3::new(target.pose().x, target.pose().y, target.pose().z),
                    visible_entities,
                })
            })
            .collect::<Vec<_>>();
        let arrow_entity_type_id = self.hostile_arrow_entity_type_id.load(Ordering::Acquire);
        let arrow_entity_type_id = (arrow_entity_type_id >= 0).then_some(arrow_entity_type_id);

        let mut creeper_fuses = Vec::new();
        let mut skeleton_attacks = Vec::new();
        let mut melee_attacks = Vec::new();
        for hostile in hostiles {
            match hostile.kind {
                HostileAttackKind::Creeper => {
                    let nearest_distance_sq = targets
                        .iter()
                        .filter(|target| target.visible_entities.contains(&hostile.id))
                        .map(|target| distance_sq(hostile.position, target.position))
                        .min_by(f64::total_cmp);
                    creeper_fuses.push(PlannedCreeperFuse {
                        hostile_id: hostile.id,
                        nearest_distance_sq,
                    });
                }
                HostileAttackKind::Skeleton => {
                    let Some(arrow_entity_type_id) = arrow_entity_type_id else {
                        continue;
                    };
                    let max_distance_sq = SKELETON_SHOT_RANGE * SKELETON_SHOT_RANGE;
                    let target = targets
                        .iter()
                        .filter_map(|target| {
                            if !target.visible_entities.contains(&hostile.id) {
                                return None;
                            }
                            let distance = distance_sq(hostile.position, target.position);
                            (distance <= max_distance_sq).then_some((distance, target.position))
                        })
                        .min_by(|left, right| left.0.total_cmp(&right.0));
                    let Some((_, target_position)) = target else {
                        continue;
                    };

                    let shooter_eye = Vec3::new(
                        hostile.position.x,
                        hostile.position.y + 1.5,
                        hostile.position.z,
                    );
                    let target_eye = Vec3::new(
                        target_position.x,
                        target_position.y + 1.0,
                        target_position.z,
                    );
                    let delta = Vec3::new(
                        target_eye.x - shooter_eye.x,
                        target_eye.y - shooter_eye.y,
                        target_eye.z - shooter_eye.z,
                    );
                    let length = (delta.x * delta.x + delta.y * delta.y + delta.z * delta.z).sqrt();
                    if length <= f64::EPSILON {
                        continue;
                    }
                    let direction = Vec3::new(delta.x / length, delta.y / length, delta.z / length);
                    let velocity = Vec3::new(
                        direction.x * SKELETON_ARROW_SPEED,
                        direction.y * SKELETON_ARROW_SPEED,
                        direction.z * SKELETON_ARROW_SPEED,
                    );
                    let position = Vec3::new(
                        shooter_eye.x + direction.x * 0.7,
                        shooter_eye.y + direction.y * 0.7,
                        shooter_eye.z + direction.z * 0.7,
                    );
                    let horizontal = velocity.x.hypot(velocity.z);
                    let yaw = velocity.z.atan2(velocity.x).to_degrees() as f32 - 90.0;
                    let pitch = (-velocity.y).atan2(horizontal).to_degrees() as f32;
                    skeleton_attacks.push(PlannedSkeletonAttack {
                        hostile_id: hostile.id,
                        arrow_entity_type_id,
                        position,
                        velocity,
                        rotation: Rotation {
                            yaw,
                            pitch,
                            head_yaw: yaw,
                        },
                    });
                }
                HostileAttackKind::Melee {
                    attack_damage: amount,
                } => {
                    if amount <= 0.0 {
                        continue;
                    }
                    let max_distance_sq = HOSTILE_MELEE_RANGE * HOSTILE_MELEE_RANGE;
                    let target = targets
                        .iter()
                        .filter_map(|target| {
                            if !target.visible_entities.contains(&hostile.id)
                                || (target.position.y - hostile.position.y).abs()
                                    > HOSTILE_MELEE_VERTICAL_REACH
                            {
                                return None;
                            }
                            let dx = target.position.x - hostile.position.x;
                            let dz = target.position.z - hostile.position.z;
                            let distance = dx * dx + dz * dz;
                            (distance <= max_distance_sq
                                && hostile_faces_target(
                                    hostile.position,
                                    hostile.rotation,
                                    target.position,
                                ))
                            .then_some((distance, target.id))
                        })
                        .min_by(|left, right| left.0.total_cmp(&right.0));
                    let Some((_, recipient)) = target else {
                        continue;
                    };
                    melee_attacks.push(PlannedMeleeAttack {
                        hostile_id: hostile.id,
                        target_session: recipient,
                        amount,
                    });
                }
            }
        }

        let creeper_ignitions = if creeper_fuses.is_empty() {
            0
        } else {
            let creeper_ids = creeper_fuses
                .iter()
                .map(|plan| plan.hostile_id)
                .collect::<HashSet<_>>();
            let mut guards = self.lock_session_entities("commit hostile creeper fuses");
            guards.entities.prefetch(&creeper_ids);
            let mut ignitions = 0;
            for plan in creeper_fuses {
                let Some(expected) = guards.entities.snapshot(plan.hostile_id) else {
                    continue;
                };
                let previous_fuse = expected.retained.primed_tnt;
                let cancel_distance_sq = CREEPER_CANCEL_RANGE * CREEPER_CANCEL_RANGE;
                let trigger_distance_sq = CREEPER_TRIGGER_RANGE * CREEPER_TRIGGER_RANGE;
                let next_fuse = match (previous_fuse, plan.nearest_distance_sq) {
                    (None, Some(distance)) if distance < trigger_distance_sq => {
                        Some(EntityPrimedTntState {
                            expires_tick: tick.saturating_add(CREEPER_FUSE_TICKS),
                            air_block_state: air.0,
                        })
                    }
                    (Some(_), Some(distance)) if distance <= cancel_distance_sq => continue,
                    (Some(fuse), _) => {
                        let remaining = fuse.expires_tick.saturating_sub(tick);
                        let progress = CREEPER_FUSE_TICKS.saturating_sub(remaining);
                        (progress > 1).then_some(EntityPrimedTntState {
                            expires_tick: fuse.expires_tick.saturating_add(2),
                            air_block_state: fuse.air_block_state,
                        })
                    }
                    (None, _) => continue,
                };
                let mut next = expected.clone();
                next.retained.primed_tnt = next_fuse;
                if guards.entities.replace_snapshot_if_current(expected, next) {
                    schedule_primed_tnt_deadline_locked(
                        &mut guards,
                        plan.hostile_id,
                        next_fuse.map(|fuse| fuse.expires_tick),
                    );
                    if previous_fuse.is_none() {
                        ignitions += 1;
                    }
                }
            }
            ignitions
        };
        if skeleton_attacks.is_empty() && melee_attacks.is_empty() {
            return (creeper_ignitions, Vec::new());
        }

        let spawned_arrows = if skeleton_attacks.is_empty() {
            Vec::new()
        } else {
            let mut entities = self.lock_entities("spawn hostile arrows ECS");
            let (hostile_ids, arrows): (Vec<_>, Vec<_>) = skeleton_attacks
                .into_iter()
                .map(|attack| {
                    let mut arrow = SpawnEntity::new(
                        attack.arrow_entity_type_id,
                        "minecraft:arrow",
                        attack.position,
                    );
                    arrow.retained.spawn_tick = tick;
                    arrow.velocity = attack.velocity;
                    arrow.rotation = attack.rotation;
                    arrow.on_ground = false;
                    apply_entity_facts(&mut arrow);
                    (attack.hostile_id, arrow)
                })
                .unzip();
            let arrow_ids = entities.spawn_batch(arrows);
            let arrow_id_set = arrow_ids.iter().copied().collect::<HashSet<_>>();
            entities.prefetch(&arrow_id_set);
            let mut spawned = Vec::with_capacity(arrow_ids.len());
            let mut transaction = Vec::with_capacity(arrow_ids.len());
            for (hostile_id, arrow_id) in hostile_ids.into_iter().zip(arrow_ids) {
                let Some(expected) = entities.snapshot(arrow_id) else {
                    continue;
                };
                let mut next = expected.clone();
                next.retained.arrow_state = Some(
                    initial_arrow_state(
                        Some(projectile_identity(hostile_id)),
                        expected.position,
                        expected.velocity,
                        expected.rotation,
                    )
                    .expect("finite hostile arrow must produce a valid kernel state"),
                );
                transaction.push((expected, next.clone()));
                spawned.push(SpawnedHostileArrow {
                    hostile_id,
                    snapshot: next,
                });
            }
            assert!(
                entities.replace_snapshots_if_current(transaction),
                "hostile arrows must retain owners before session publication"
            );
            spawned
        };
        #[cfg(test)]
        self.pause_between_hostile_entity_and_session_commit_for_test();

        let mut attacks = 0;
        let mut dispatches = Vec::new();
        let hostile_by_arrow = spawned_arrows
            .iter()
            .map(|arrow| (arrow.snapshot.id, arrow.hostile_id))
            .collect::<HashMap<_, _>>();
        let spawned_arrows = self
            .current_expected_entity_snapshots(
                spawned_arrows.into_iter().map(|arrow| arrow.snapshot),
            )
            .into_iter()
            .map(|snapshot| SpawnedHostileArrow {
                hostile_id: hostile_by_arrow[&snapshot.id],
                snapshot,
            })
            .collect::<Vec<_>>();
        if !spawned_arrows.is_empty() {
            let mut inner = self.lock_inner("publish hostile arrows");
            for arrow in spawned_arrows {
                let snapshot = server_entity_snapshot_from(arrow.snapshot);
                let arrow_id = snapshot.id;
                let arrow_position = snapshot.position;
                let arrow_type_id = snapshot.type_id;
                inner
                    .entity_type_aabbs
                    .entry(arrow_type_id)
                    .or_insert_with(|| entity_aabb(&snapshot.type_name));
                track_entity_chunk_locked(&mut inner, arrow_id, arrow_position);
                initialize_entity_wire_state_from_snapshot_locked(&mut inner, &snapshot);
                dispatches.extend(spawn_entity_visibility_from_snapshot_locked(
                    &mut inner, snapshot,
                ));
                let animation_recipients = session_recipients(
                    &inner,
                    visible_entity_observers_locked(&inner, arrow.hostile_id),
                );
                dispatches.extend(visibility_dispatches(animation_recipients, || {
                    OutboundCommand::AnimatePlayer {
                        entity_id: arrow.hostile_id.0,
                    }
                }));
                attacks += 1;
            }
        }

        if !melee_attacks.is_empty() {
            let melee_ids = melee_attacks
                .iter()
                .map(|attack| attack.hostile_id)
                .collect::<HashSet<_>>();
            let current_hostiles = {
                let entities = self.lock_entities("validate hostile melee attackers");
                entities.prefetch(&melee_ids);
                melee_ids
                    .iter()
                    .filter_map(|&entity_id| {
                        entities
                            .snapshot(entity_id)
                            .map(|entity| (entity_id, entity))
                    })
                    .collect::<HashMap<_, _>>()
            };
            #[cfg(test)]
            self.pause_before_hostile_session_publication_for_test();
            let recipients = self.movement_recipients.load_full();
            let mut reserved_attacks = Vec::with_capacity(melee_attacks.len());
            for attack in melee_attacks {
                let Some(hostile) = current_hostiles.get(&attack.hostile_id) else {
                    continue;
                };
                if hostile.lifecycle != EntityLifecycle::Alive {
                    continue;
                }
                let Some(target_publication) = recipients.get(&attack.target_session) else {
                    continue;
                };
                let Some((_, target_recipient)) =
                    target_publication.reserve_combat_recipient_if(|target, visible_entities| {
                        #[cfg(test)]
                        self.pause_after_hostile_target_snapshot_for_test();
                        let target_pose = target.pose();
                        if !target.is_targetable()
                            || !visible_entities.contains(&attack.hostile_id)
                            || (target_pose.y - hostile.position.y).abs()
                                > HOSTILE_MELEE_VERTICAL_REACH
                        {
                            return false;
                        }
                        let dx = target_pose.x - hostile.position.x;
                        let dz = target_pose.z - hostile.position.z;
                        dx * dx + dz * dz <= HOSTILE_MELEE_RANGE * HOSTILE_MELEE_RANGE
                            && hostile_faces_target(
                                hostile.position,
                                hostile.rotation,
                                Vec3::new(target_pose.x, target_pose.y, target_pose.z),
                            )
                    })
                else {
                    continue;
                };
                reserved_attacks.push((attack, hostile.clone(), target_recipient));
            }
            let current_attacker_ids = self
                .current_expected_entity_snapshots(
                    reserved_attacks
                        .iter()
                        .map(|(_, hostile, _)| hostile.clone()),
                )
                .into_iter()
                .map(|hostile| hostile.id)
                .collect::<HashSet<_>>();
            for (attack, hostile, target_recipient) in reserved_attacks {
                if !current_attacker_ids.contains(&attack.hostile_id) {
                    continue;
                }
                dispatches.push(VisibilityDispatch {
                    recipient: target_recipient,
                    command: OutboundCommand::DamagePlayer {
                        damage: PlayerDamageRequest {
                            kind: PlayerDamageKind::MobAttack,
                            amount: attack.amount,
                            source_origin: Some(hostile.position),
                        },
                    },
                });
                let animation_recipients = recipients
                    .values()
                    .filter_map(|publication| {
                        publication.reserve_observer_if_visible(attack.hostile_id)
                    })
                    .collect::<Vec<_>>();
                dispatches.extend(visibility_dispatches(animation_recipients, || {
                    OutboundCommand::AnimatePlayer {
                        entity_id: attack.hostile_id.0,
                    }
                }));
                attacks += 1;
            }
        }

        (attacks + creeper_ignitions, dispatches)
    }

    #[cfg(test)]
    pub(super) fn hostile_attack_candidate_count(&self) -> u64 {
        self.hostile_attack_candidates.load(Ordering::Relaxed)
    }

    pub(in crate::play) fn has_rest_preventing_hostile_near_bed(
        &self,
        bed: mc_world::BlockPos,
    ) -> bool {
        const HORIZONTAL_RANGE: f64 = 8.0;
        const VERTICAL_RANGE: f64 = 5.0;

        let center = Vec3::new(
            f64::from(bed.x) + 0.5,
            f64::from(bed.y),
            f64::from(bed.z) + 0.5,
        );
        let inner = self.lock_session_entities("check monsters near bed");
        nearby_entity_candidate_ids_locked(&inner, center, HORIZONTAL_RANGE + 1.0)
            .into_iter()
            .filter_map(|id| inner.entities.snapshot(id))
            .any(|entity| {
                let aabb = entity_aabb(&entity.type_name);
                entity.lifecycle == EntityLifecycle::Alive
                    && entity.item_stack.is_none()
                    && is_hostile_entity(&entity.type_name)
                    && entity.position.x + aabb.half_width >= center.x - HORIZONTAL_RANGE
                    && entity.position.x - aabb.half_width <= center.x + HORIZONTAL_RANGE
                    && entity.position.y + aabb.height >= center.y - VERTICAL_RANGE
                    && entity.position.y <= center.y + VERTICAL_RANGE
                    && entity.position.z + aabb.half_width >= center.z - HORIZONTAL_RANGE
                    && entity.position.z - aabb.half_width <= center.z + HORIZONTAL_RANGE
            })
    }

    #[cfg(test)]
    pub(in crate::play) fn nearby_hostile_entities(
        &self,
        position: Vec3,
        radius: f64,
    ) -> Vec<ServerEntitySnapshot> {
        let inner = self.lock_session_entities("nearby hostile entities");
        nearby_entity_snapshots_locked(&inner, position, radius, |entity| {
            entity.item_stack.is_none() && is_hostile_entity(&entity.type_name)
        })
    }
}

fn hostile_faces_target(position: Vec3, rotation: Rotation, target: Vec3) -> bool {
    let dx = target.x - position.x;
    let dz = target.z - position.z;
    let distance = dx.hypot(dz);
    if distance <= f64::EPSILON {
        return true;
    }
    if !rotation.head_yaw.is_finite() {
        return false;
    }
    let yaw = f64::from(rotation.head_yaw).to_radians();
    let facing_x = -yaw.sin();
    let facing_z = yaw.cos();
    (facing_x * dx + facing_z * dz) / distance > 0.0
}

pub(super) fn update_hostile_targets(
    entities: &mut EntityOwnerAccess,
    players: &[Vec3],
    active_ids: Option<&HashSet<EntityId>>,
    mob_behaviors: &MobBehaviorTable,
) {
    let mut hostiles = Vec::new();
    let mut collect_hostile = |entity: mc_entity::EntityView<'_>| {
        if entity.lifecycle == EntityLifecycle::Alive && is_hostile_entity(entity.type_name) {
            let Some(profile) = mob_behaviors.get_by_name(entity.type_name) else {
                return;
            };
            let follow_range = entity
                .attributes
                .base(&AttributeKind::FollowRange)
                .unwrap_or(16.0);
            hostiles.push((
                entity.id,
                entity.position,
                follow_range,
                matches!(
                    profile.combat,
                    MobCombatPolicy::Arrow | MobCombatPolicy::UnsupportedSpecial
                ),
                profile.combat == MobCombatPolicy::CreeperFuse,
                entity.retained.primed_tnt.is_some(),
                profile.wander_speed,
                profile.wander_period_ticks,
                profile.pursuit_speed,
                entity.goal.clone(),
            ));
        }
    };
    if let Some(active_ids) = active_ids {
        entities.visit_simulation_entities_for_ids(active_ids, &mut collect_hostile);
    } else {
        entities.visit_simulation_entities(&mut collect_hostile);
    }
    if players.is_empty() {
        let changed = hostiles
            .into_iter()
            .filter_map(
                |(
                    hostile_id,
                    _,
                    _,
                    _,
                    is_creeper,
                    fuse_active,
                    wander_speed,
                    wander_period_ticks,
                    _,
                    current,
                )| {
                    let goal = if is_creeper && fuse_active {
                        GoalState::Idle
                    } else {
                        hostile_wander_goal_for(wander_speed, wander_period_ticks)
                    };
                    changed_hostile_goal(hostile_id, &current, goal)
                },
            )
            .collect::<Vec<_>>();
        if !changed.is_empty() {
            let _ = entities.set_goals_deferred_journal(changed);
        }
        return;
    }
    let changed = hostiles
        .into_iter()
        .filter_map(
            |(
                hostile_id,
                hostile_position,
                follow_range,
                uses_ranged_attack,
                is_creeper,
                fuse_active,
                wander_speed,
                wander_period_ticks,
                pursuit_speed,
                current,
            )| {
                let max_distance_sq = follow_range * follow_range;
                let target = players
                    .iter()
                    .copied()
                    .filter(|position| distance_sq(*position, hostile_position) <= max_distance_sq)
                    .min_by(|left, right| {
                        distance_sq(*left, hostile_position)
                            .total_cmp(&distance_sq(*right, hostile_position))
                    });
                let goal = match target {
                    None if is_creeper && fuse_active => GoalState::Idle,
                    None => hostile_wander_goal_for(wander_speed, wander_period_ticks),
                    Some(target)
                        if is_creeper
                            && (fuse_active
                                || distance_sq(target, hostile_position)
                                    < CREEPER_TRIGGER_RANGE * CREEPER_TRIGGER_RANGE) =>
                    {
                        GoalState::Idle
                    }
                    Some(target)
                        if !uses_ranged_attack
                            && (target.y - hostile_position.y).abs()
                                <= HOSTILE_MELEE_VERTICAL_REACH
                            && (target.x - hostile_position.x).powi(2)
                                + (target.z - hostile_position.z).powi(2)
                                <= HOSTILE_MELEE_RANGE * HOSTILE_MELEE_RANGE =>
                    {
                        GoalState::FollowPosition { target, speed: 0.0 }
                    }
                    Some(target) => GoalState::FollowPosition {
                        target,
                        speed: pursuit_speed,
                    },
                };
                changed_hostile_goal(hostile_id, &current, goal)
            },
        )
        .collect::<Vec<_>>();
    if !changed.is_empty() {
        let _ = entities.set_goals_deferred_journal(changed);
    }
}

#[cfg(test)]
pub(super) fn hostile_wander_goal() -> GoalState {
    hostile_wander_goal_for(HOSTILE_FOLLOW_SPEED, 20)
}

fn hostile_wander_goal_for(speed: f64, period_ticks: u32) -> GoalState {
    GoalState::Wander {
        speed,
        period_ticks,
    }
}

pub(super) fn changed_hostile_goal(
    entity: EntityId,
    current: &GoalState,
    next: GoalState,
) -> Option<(EntityId, GoalState)> {
    (current != &next).then_some((entity, next))
}
