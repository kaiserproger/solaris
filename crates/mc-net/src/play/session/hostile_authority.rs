use std::collections::{HashMap, HashSet};
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::Ordering;

use mc_entity::{
    AttributeKind, EntityId, EntityLifecycle, EntityPrimedTntState, EntitySnapshot, GoalState,
    Rotation, SpawnEntity, Vec3,
};
use mc_world::BlockStateId;

use crate::play::combat::{PlayerDamageKind, PlayerDamageRequest};
use crate::play::simulation::SimulationAuthority;
use crate::play::{
    CREEPER_CANCEL_RANGE, CREEPER_FUSE_TICKS, CREEPER_TRIGGER_RANGE, HOSTILE_FOLLOW_SPEED,
    HOSTILE_MELEE_PERIOD_TICKS, HOSTILE_MELEE_RANGE, HOSTILE_MELEE_VERTICAL_REACH,
    SKELETON_ARROW_SPEED, SKELETON_SHOT_PERIOD_TICKS, SKELETON_SHOT_RANGE,
};

#[cfg(test)]
use super::entity_lifecycle::nearby_entity_snapshots_locked;
use super::entity_lifecycle::{nearby_entity_candidate_ids_locked, track_entity_chunk_locked};
use super::entity_owner::EntityOwnerAccess;
use super::interaction_geometry::{distance_sq, entity_aabb};
#[cfg(test)]
use super::outbound::ServerEntitySnapshot;
use super::outbound::{OutboundCommand, SessionRecipient, VisibilityDispatch};
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
    source_origin: Vec3,
    recipient: SessionRecipient,
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

    pub(in crate::play) fn tick_hostile_attacks(
        &self,
        _authority: &SimulationAuthority,
        tick: u64,
        air: BlockStateId,
    ) -> (usize, Vec<VisibilityDispatch>) {
        let loaded_entity_ids = {
            let inner = self.lock_inner("snapshot loaded hostile candidates");
            inner
                .loaded_chunk_refcounts
                .keys()
                .filter_map(|chunk| inner.entities_by_chunk.get(chunk))
                .flat_map(|entities| entities.iter().copied())
                .filter(|entity_id| inner.hostile_entities.contains(entity_id))
                .collect::<HashSet<_>>()
        };
        if loaded_entity_ids.is_empty() {
            return (0, Vec::new());
        }
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
                let kind = match entity.type_name {
                    "minecraft:creeper" => HostileAttackKind::Creeper,
                    "minecraft:skeleton" => HostileAttackKind::Skeleton,
                    entity_type if is_hostile_entity(entity_type) => HostileAttackKind::Melee {
                        attack_damage: entity
                            .attributes
                            .base(&AttributeKind::AttackDamage)
                            .unwrap_or(3.0) as f32,
                    },
                    _ => return,
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
                });
            });
        }
        #[cfg(test)]
        self.hostile_attack_candidates
            .fetch_add(hostiles.len() as u64, Ordering::Relaxed);
        if hostiles.is_empty() {
            return (0, Vec::new());
        }

        let (skeleton_attacks, melee_attacks, creeper_ignitions) = {
            let mut inner = self.lock_session_entities("plan hostile attacks");
            let mut skeleton_attacks = Vec::new();
            let mut melee_attacks = Vec::new();
            let mut creeper_ignitions = 0;
            for hostile in hostiles {
                match hostile.kind {
                    HostileAttackKind::Creeper => {
                        let cancel_distance_sq = CREEPER_CANCEL_RANGE * CREEPER_CANCEL_RANGE;
                        let trigger_distance_sq = CREEPER_TRIGGER_RANGE * CREEPER_TRIGGER_RANGE;
                        let nearest_distance_sq = inner
                            .sessions
                            .iter()
                            .filter_map(|(&session_id, session)| {
                                if inner.spectator_sessions.contains(&session_id)
                                    || !session.visible_entities.contains(&hostile.id)
                                {
                                    return None;
                                }
                                let position =
                                    Vec3::new(session.pose.x, session.pose.y, session.pose.z);
                                Some(distance_sq(hostile.position, position))
                            })
                            .min_by(f64::total_cmp);
                        let Some(expected) = inner.entities.snapshot(hostile.id) else {
                            continue;
                        };
                        let previous_fuse = expected.retained.primed_tnt;
                        let next_fuse = match (previous_fuse, nearest_distance_sq) {
                            (None, Some(distance)) if distance < trigger_distance_sq => {
                                Some(EntityPrimedTntState {
                                    expires_tick: tick.saturating_add(CREEPER_FUSE_TICKS),
                                    air_block_state: air.0,
                                })
                            }
                            (Some(_), Some(distance)) if distance <= cancel_distance_sq => {
                                continue;
                            }
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
                        if inner.entities.replace_snapshot_if_current(expected, next)
                            && previous_fuse.is_none()
                        {
                            creeper_ignitions += 1;
                        }
                    }
                    HostileAttackKind::Skeleton => {
                        let Some(arrow_entity_type_id) =
                            inner.arrow_kill_rewards.arrow_entity_type_id
                        else {
                            continue;
                        };
                        let max_distance_sq = SKELETON_SHOT_RANGE * SKELETON_SHOT_RANGE;
                        let target = inner
                            .sessions
                            .values()
                            .filter_map(|session| {
                                if !session.visible_entities.contains(&hostile.id) {
                                    return None;
                                }
                                let position =
                                    Vec3::new(session.pose.x, session.pose.y, session.pose.z);
                                let distance = distance_sq(hostile.position, position);
                                (distance <= max_distance_sq).then_some((distance, position))
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
                        let length =
                            (delta.x * delta.x + delta.y * delta.y + delta.z * delta.z).sqrt();
                        if length <= f64::EPSILON {
                            continue;
                        }
                        let direction =
                            Vec3::new(delta.x / length, delta.y / length, delta.z / length);
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
                        let target = inner
                            .sessions
                            .iter()
                            .filter_map(|(&session_id, session)| {
                                if !session.visible_entities.contains(&hostile.id)
                                    || (session.pose.y - hostile.position.y).abs()
                                        > HOSTILE_MELEE_VERTICAL_REACH
                                {
                                    return None;
                                }
                                let dx = session.pose.x - hostile.position.x;
                                let dz = session.pose.z - hostile.position.z;
                                let distance = dx * dx + dz * dz;
                                (distance <= max_distance_sq).then(|| {
                                    (
                                        distance,
                                        SessionRecipient::unordered(
                                            session_id,
                                            session.tx.clone(),
                                            Arc::clone(&session.pressure),
                                        ),
                                    )
                                })
                            })
                            .min_by(|left, right| left.0.total_cmp(&right.0));
                        let Some((_, recipient)) = target else {
                            continue;
                        };
                        melee_attacks.push(PlannedMeleeAttack {
                            hostile_id: hostile.id,
                            source_origin: hostile.position,
                            recipient,
                            amount,
                        });
                    }
                }
            }
            (skeleton_attacks, melee_attacks, creeper_ignitions)
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

        let mut inner = self.lock_inner("publish hostile attacks");
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
            });
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
        for attack in melee_attacks {
            dispatches.push(VisibilityDispatch {
                recipient: attack.recipient,
                command: OutboundCommand::DamagePlayer {
                    damage: PlayerDamageRequest {
                        kind: PlayerDamageKind::MobAttack,
                        amount: attack.amount,
                        source_origin: Some(attack.source_origin),
                    },
                },
            });
            let animation_recipients = session_recipients(
                &inner,
                visible_entity_observers_locked(&inner, attack.hostile_id),
            );
            dispatches.extend(visibility_dispatches(animation_recipients, || {
                OutboundCommand::AnimatePlayer {
                    entity_id: attack.hostile_id.0,
                }
            }));
            attacks += 1;
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

pub(super) fn update_hostile_targets(
    entities: &mut EntityOwnerAccess,
    players: &[Vec3],
    active_ids: Option<&HashSet<EntityId>>,
) {
    let mut hostiles = Vec::new();
    let mut collect_hostile = |entity: mc_entity::EntityView<'_>| {
        if entity.lifecycle == EntityLifecycle::Alive && is_hostile_entity(entity.type_name) {
            let follow_range = entity
                .attributes
                .base(&AttributeKind::FollowRange)
                .unwrap_or(16.0);
            hostiles.push((
                entity.id,
                entity.position,
                follow_range,
                entity.type_name == "minecraft:skeleton",
                entity.type_name == "minecraft:creeper",
                entity.retained.primed_tnt.is_some(),
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
            .filter_map(|(hostile_id, _, _, _, is_creeper, fuse_active, current)| {
                let goal = if is_creeper && fuse_active {
                    GoalState::Idle
                } else {
                    hostile_wander_goal()
                };
                changed_hostile_goal(hostile_id, &current, goal)
            })
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
                    None => hostile_wander_goal(),
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
                        GoalState::Idle
                    }
                    Some(target) => GoalState::FollowPosition {
                        target,
                        speed: HOSTILE_FOLLOW_SPEED,
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

pub(super) fn hostile_wander_goal() -> GoalState {
    GoalState::Wander {
        speed: HOSTILE_FOLLOW_SPEED,
        period_ticks: 20,
    }
}

pub(super) fn changed_hostile_goal(
    entity: EntityId,
    current: &GoalState,
    next: GoalState,
) -> Option<(EntityId, GoalState)> {
    (current != &next).then_some((entity, next))
}
