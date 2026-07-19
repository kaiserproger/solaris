use std::collections::HashMap;
use std::sync::Arc;

use mc_entity::{
    EntityId, EntityLifecycle, EntityMotionState, EntitySnapshot, Rotation, SpawnEntity, Vec3,
};

use crate::play::combat::{PlayerDamageKind, PlayerDamageRequest};
use crate::play::spawn::chunk_pos_from_coords;
use crate::play::survival::{entity_item_stack, mob_xp_value};
use crate::play::{
    ARROW_DESPAWN_AGE_TICKS, ARROW_ENTITY_HIT_DAMAGE, ARROW_ENTITY_HIT_KNOCKBACK, EntityPhysicsStep,
};

use super::entity_combat::{begin_server_entity_death_locked, damage_server_entity_locked};
use super::entity_lifecycle::{remove_server_entity_locked, track_entity_chunk_locked};
use super::interaction_geometry::{entity_aabb, entity_geometry};
use super::outbound::{OutboundCommand, ServerEntityMove, SessionRecipient, VisibilityDispatch};
use super::visibility::{
    entity_event_dispatches_locked, initialize_entity_wire_state_locked, ordered_session_recipient,
    publish_server_entity_snapshot_locked, spawn_entity_visibility_locked,
    visible_entity_observers_locked,
};
use super::{
    EntityKillRewards, SessionEntityGuards, SessionId, apply_entity_facts, entity_kill_drop_stacks,
    player_aabb, player_collision_position,
};

pub(super) fn spawn_arrow_locked(
    inner: &mut SessionEntityGuards<'_>,
    owner_session: Option<SessionId>,
    entity_type_id: i32,
    position: Vec3,
    velocity: Vec3,
    rotation: Rotation,
) -> (EntityId, Vec<VisibilityDispatch>) {
    let mut entity = SpawnEntity::new(entity_type_id, "minecraft:arrow", position);
    entity.velocity = velocity;
    entity.rotation = rotation;
    entity.on_ground = false;
    apply_entity_facts(&mut entity);
    let aabb = entity_aabb(&entity.type_name);
    let id = inner.entities.spawn_authoritative(entity);
    let lifecycle_tick = inner.entity_lifecycle_tick;
    inner.entity_spawn_ticks.insert(id, lifecycle_tick);
    inner.arrow_spawn_ticks.insert(id, lifecycle_tick);
    if let Some(owner_session) = owner_session {
        inner.arrow_owner_sessions.insert(id, owner_session);
    }
    inner
        .entity_type_aabbs
        .entry(entity_type_id)
        .or_insert(aabb);
    track_entity_chunk_locked(inner, id, position);
    initialize_entity_wire_state_locked(inner, id);
    let dispatches = spawn_entity_visibility_locked(inner, id);
    (id, dispatches)
}

pub(super) fn despawn_expired_arrows_locked(
    inner: &mut SessionEntityGuards<'_>,
) -> Vec<VisibilityDispatch> {
    let expired = inner
        .arrow_spawn_ticks
        .iter()
        .filter_map(|(&entity_id, &spawn_tick)| {
            (inner.entity_lifecycle_tick.saturating_sub(spawn_tick) >= ARROW_DESPAWN_AGE_TICKS)
                .then_some(entity_id)
        })
        .collect::<Vec<_>>();
    expired
        .into_iter()
        .filter_map(|entity_id| {
            remove_server_entity_locked(inner, entity_id).map(|(_, dispatches)| dispatches)
        })
        .flatten()
        .collect()
}

pub(super) fn resolve_arrow_entity_hits_locked(
    inner: &mut SessionEntityGuards<'_>,
    steps: &[EntityPhysicsStep],
    old_motion: &HashMap<EntityId, EntityMotionState>,
) -> Vec<VisibilityDispatch> {
    let mut dispatches = Vec::new();
    for step in steps {
        let Some(motion) = old_motion.get(&step.id) else {
            continue;
        };
        if !motion.is_arrow || !inner.entities.contains(step.id) {
            continue;
        }
        let start = motion.position;
        let Some(target) = first_arrow_hit_target_locked(inner, step.id, start, step.position)
        else {
            continue;
        };
        match target {
            ArrowHitTarget::Entity(target_id) => {
                let Some(damage) =
                    damage_server_entity_locked(inner, target_id, ARROW_ENTITY_HIT_DAMAGE)
                else {
                    continue;
                };
                if damage.killed {
                    let rewards = EntityKillRewards {
                        items: inner.arrow_kill_rewards.item_entity_type_id.map_or_else(
                            Vec::new,
                            |entity_type_id| {
                                entity_kill_drop_stacks(
                                    &inner.arrow_kill_rewards,
                                    &damage.snapshot.type_name,
                                    damage.snapshot.animal,
                                    damage.snapshot.id.0 as i64 as u64,
                                )
                                .into_iter()
                                .map(|drop| (entity_type_id, entity_item_stack(drop)))
                                .collect()
                            },
                        ),
                        experience: inner.arrow_kill_rewards.xp_orb_entity_type_id.map(
                            |entity_type_id| {
                                (entity_type_id, mob_xp_value(&damage.snapshot.type_name))
                            },
                        ),
                    };
                    let (_, target_dispatches) =
                        begin_server_entity_death_locked(inner, &damage, &rewards);
                    dispatches.extend(target_dispatches);
                } else {
                    dispatches.extend(apply_arrow_knockback_locked(
                        inner,
                        target_id,
                        start,
                        step.position,
                    ));
                    dispatches.extend(entity_event_dispatches_locked(inner, target_id, 2));
                }
            }
            ArrowHitTarget::Player(session_id) => {
                if let Some(session) = inner.sessions.get(&session_id) {
                    dispatches.push(VisibilityDispatch {
                        recipient: SessionRecipient::unordered(
                            session_id,
                            session.tx.clone(),
                            Arc::clone(&session.pressure),
                        ),
                        command: OutboundCommand::DamagePlayer {
                            damage: PlayerDamageRequest {
                                kind: PlayerDamageKind::Projectile,
                                amount: ARROW_ENTITY_HIT_DAMAGE,
                                source_origin: Some(start),
                            },
                        },
                    });
                }
            }
        }
        if let Some((_, arrow_dispatches)) = remove_server_entity_locked(inner, step.id) {
            dispatches.extend(arrow_dispatches);
        }
    }
    dispatches
}

fn apply_arrow_knockback_locked(
    inner: &mut SessionEntityGuards<'_>,
    target_id: EntityId,
    start: Vec3,
    end: Vec3,
) -> Vec<VisibilityDispatch> {
    let Some(knockback) = arrow_knockback(start, end) else {
        return Vec::new();
    };
    let Some(target) = inner.entities.snapshot(target_id) else {
        return Vec::new();
    };
    let velocity = Vec3::new(
        target.velocity.x + knockback.x,
        (target.velocity.y + knockback.y).max(knockback.y),
        target.velocity.z + knockback.z,
    );
    if !inner.entities.set_velocity(target_id, velocity) {
        return Vec::new();
    }
    let snapshot = if let Some(snapshot) = inner.published_entity_snapshots.get_mut(&target_id) {
        snapshot.velocity = velocity;
        snapshot.clone()
    } else {
        let Some(snapshot) = publish_server_entity_snapshot_locked(inner, target_id) else {
            return Vec::new();
        };
        snapshot
    };
    visible_entity_observers_locked(inner, target_id)
        .into_iter()
        .filter_map(|observer_id| {
            let observer = inner.sessions.get(&observer_id)?;
            Some(VisibilityDispatch {
                recipient: ordered_session_recipient(observer_id, observer),
                command: OutboundCommand::MoveEntityRelative(ServerEntityMove {
                    id: target_id,
                    delta: Vec3::ZERO,
                    velocity: snapshot.velocity,
                    rotation: snapshot.rotation,
                    on_ground: snapshot.on_ground,
                    send_position_rotation: false,
                    send_velocity: true,
                }),
            })
        })
        .collect()
}

fn arrow_knockback(start: Vec3, end: Vec3) -> Option<Vec3> {
    let dx = end.x - start.x;
    let dz = end.z - start.z;
    let horizontal = dx.hypot(dz);
    if horizontal <= f64::EPSILON {
        return None;
    }
    Some(Vec3::new(
        dx / horizontal * ARROW_ENTITY_HIT_KNOCKBACK,
        0.1,
        dz / horizontal * ARROW_ENTITY_HIT_KNOCKBACK,
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArrowHitTarget {
    Entity(EntityId),
    Player(SessionId),
}

fn first_arrow_hit_target_locked(
    inner: &SessionEntityGuards<'_>,
    arrow_id: EntityId,
    start: Vec3,
    end: Vec3,
) -> Option<ArrowHitTarget> {
    let owner = inner.arrow_owner_sessions.get(&arrow_id).copied();
    let owner_entity = inner.arrow_owner_entities.get(&arrow_id).copied();
    let entity_hit = arrow_entity_candidate_ids_locked(inner, start, end)
        .into_iter()
        .filter_map(|entity_id| inner.entities.snapshot(entity_id))
        .filter(|entity| Some(entity.id) != owner_entity)
        .filter(|entity| arrow_can_hit_entity(arrow_id, entity))
        .filter_map(|entity| {
            let aabb = entity_geometry(&entity.type_name, entity.animal).aabb;
            segment_target_aabb_t(start, end, entity.position, aabb)
                .map(|t| (t, ArrowHitTarget::Entity(entity.id)))
        })
        .min_by(|(left_t, _), (right_t, _)| left_t.total_cmp(right_t));
    let player_hit = inner
        .sessions
        .iter()
        .filter(|(session_id, _)| Some(**session_id) != owner)
        .filter_map(|(session_id, session)| {
            segment_target_aabb_t(
                start,
                end,
                player_collision_position(session.pose),
                player_aabb(),
            )
            .map(|t| (t, ArrowHitTarget::Player(*session_id)))
        })
        .min_by(|(left_t, _), (right_t, _)| left_t.total_cmp(right_t));
    entity_hit
        .into_iter()
        .chain(player_hit)
        .min_by(|(left_t, _), (right_t, _)| left_t.total_cmp(right_t))
        .map(|(_, target)| target)
}

const ARROW_HIT_EXPANSION: f64 = 0.25;

pub(super) fn arrow_entity_candidate_ids_locked(
    inner: &SessionEntityGuards<'_>,
    start: Vec3,
    end: Vec3,
) -> Vec<EntityId> {
    if [start.x, start.z, end.x, end.z]
        .into_iter()
        .any(|coordinate| !coordinate.is_finite())
    {
        return Vec::new();
    }
    let max_half_width = inner
        .entity_type_aabbs
        .values()
        .map(|aabb| aabb.half_width)
        .filter(|half_width| half_width.is_finite() && *half_width >= 0.0)
        .fold(0.0_f64, f64::max);
    let padding = max_half_width + ARROW_HIT_EXPANSION;
    let (min_cx, min_cz) =
        chunk_pos_from_coords(start.x.min(end.x) - padding, start.z.min(end.z) - padding);
    let (max_cx, max_cz) =
        chunk_pos_from_coords(start.x.max(end.x) + padding, start.z.max(end.z) + padding);
    let chunk_count = u64::from(min_cx.abs_diff(max_cx))
        .saturating_add(1)
        .saturating_mul(u64::from(min_cz.abs_diff(max_cz)).saturating_add(1));
    if chunk_count >= inner.entities_by_chunk.len() as u64 {
        let mut ids = inner.entity_chunks.keys().copied().collect::<Vec<_>>();
        ids.sort_unstable();
        return ids;
    }

    let mut ids = Vec::new();
    for cz in min_cz..=max_cz {
        for cx in min_cx..=max_cx {
            if let Some(chunk_ids) = inner.entities_by_chunk.get(&(cx, cz)) {
                ids.extend(chunk_ids.iter().copied());
            }
        }
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}

fn arrow_can_hit_entity(arrow_id: EntityId, entity: &EntitySnapshot) -> bool {
    entity.id != arrow_id
        && entity.lifecycle == EntityLifecycle::Alive
        && entity.type_name != "minecraft:arrow"
        && entity.item_stack.is_none()
        && entity.experience_value.is_none()
        && entity.block_state.is_none()
}

fn segment_target_aabb_t(
    start: Vec3,
    end: Vec3,
    target_position: Vec3,
    target_aabb: mc_physics::Aabb,
) -> Option<f64> {
    let min = Vec3::new(
        target_position.x - target_aabb.half_width - ARROW_HIT_EXPANSION,
        target_position.y - ARROW_HIT_EXPANSION,
        target_position.z - target_aabb.half_width - ARROW_HIT_EXPANSION,
    );
    let max = Vec3::new(
        target_position.x + target_aabb.half_width + ARROW_HIT_EXPANSION,
        target_position.y + target_aabb.height + ARROW_HIT_EXPANSION,
        target_position.z + target_aabb.half_width + ARROW_HIT_EXPANSION,
    );
    segment_aabb_intersection_t(start, end, min, max)
}

pub(super) fn segment_aabb_intersection_t(
    start: Vec3,
    end: Vec3,
    min: Vec3,
    max: Vec3,
) -> Option<f64> {
    if [
        start.x, start.y, start.z, end.x, end.y, end.z, min.x, min.y, min.z, max.x, max.y, max.z,
    ]
    .into_iter()
    .any(|coordinate| !coordinate.is_finite())
        || min.x > max.x
        || min.y > max.y
        || min.z > max.z
    {
        return None;
    }
    let delta = Vec3::new(end.x - start.x, end.y - start.y, end.z - start.z);
    if [delta.x, delta.y, delta.z]
        .into_iter()
        .any(|coordinate| !coordinate.is_finite())
    {
        return None;
    }
    let mut t_min: f64 = 0.0;
    let mut t_max: f64 = 1.0;
    for (origin, direction, min_axis, max_axis) in [
        (start.x, delta.x, min.x, max.x),
        (start.y, delta.y, min.y, max.y),
        (start.z, delta.z, min.z, max.z),
    ] {
        if direction.abs() <= f64::EPSILON {
            if origin < min_axis || origin > max_axis {
                return None;
            }
            continue;
        }
        let inv_direction = 1.0 / direction;
        let mut near = (min_axis - origin) * inv_direction;
        let mut far = (max_axis - origin) * inv_direction;
        if near > far {
            std::mem::swap(&mut near, &mut far);
        }
        t_min = t_min.max(near);
        t_max = t_max.min(far);
        if t_min > t_max {
            return None;
        }
    }
    Some(t_min.clamp(0.0, 1.0))
}
