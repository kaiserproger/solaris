use std::collections::{HashMap, HashSet};

use mc_entity::{EntityId, EntityLifecycle, SpawnEntity, Vec3};
use mc_world::BlockStateId;

use crate::play::PlayerPose;
use crate::play::combat::{PlayerDamageKind, PlayerDamageRequest};
use crate::play::explosions::{PlayerExplosionImpact, TNT_ENTITY_TYPE_NAME, tnt_explosion_packet};
use crate::play::simulation::SimulationAuthority;
use crate::play::survival::{entity_item_stack, mob_xp_value};

use super::entity_combat::attack_server_entity_locked;
use super::entity_lifecycle::{
    nearby_entity_candidate_ids_locked, remove_server_entity_state_locked,
    track_entity_chunk_locked,
};
use super::interaction_geometry::{distance_sq, entity_aabb, entity_geometry};
use super::outbound::{
    OutboundCommand, ServerEntityMove, ServerEntitySnapshot, VisibilityDispatch,
};
use super::visibility::{
    entity_event_dispatches_locked, initialize_entity_wire_state_locked, ordered_session_recipient,
    publish_server_entity_snapshot_locked, remove_entity_visibility_locked,
    spawn_entity_visibility_locked, visible_entity_observers_locked,
};
use super::{
    EntityAttackOutcome, EntityKillRewards, SessionEntityGuards, SessionId, SessionRegistry,
    SessionRegistryInner, entity_kill_drop_stacks, player_collision_position,
    record_entity_dispatches_locked,
};

pub(in crate::play) struct ExpiredPrimedTnt {
    pub(in crate::play) entity_id: EntityId,
    pub(in crate::play) position: Vec3,
    pub(in crate::play) entity_type_id: i32,
    pub(in crate::play) air: mc_world::BlockStateId,
    snapshot: ServerEntitySnapshot,
    observer_ids: Vec<SessionId>,
    explosion_targets: Vec<ExplosionPlayerTarget>,
}

#[derive(Debug, Clone)]
pub(in crate::play) struct ExplosionPlayerTarget {
    pub(in crate::play) session_id: SessionId,
    pub(in crate::play) pose: PlayerPose,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::play) struct ExplosionEntityTarget {
    pub(in crate::play) entity_id: EntityId,
    pub(in crate::play) position: Vec3,
    pub(in crate::play) eye_position: Vec3,
    pub(in crate::play) aabb_min: Vec3,
    pub(in crate::play) aabb_max: Vec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::play) struct ServerEntityExplosionImpact {
    pub(in crate::play) entity_id: EntityId,
    pub(in crate::play) damage: f32,
    pub(in crate::play) knockback: Vec3,
}

impl ExpiredPrimedTnt {
    pub(in crate::play) fn explosion_targets(&self) -> &[ExplosionPlayerTarget] {
        &self.explosion_targets
    }

    fn plan_dispatches(
        self,
        inner: &mut SessionRegistryInner,
        block_count: i32,
        impacts: &HashMap<SessionId, PlayerExplosionImpact>,
    ) -> Vec<VisibilityDispatch> {
        let observer_ids = self.observer_ids.into_iter().collect::<HashSet<_>>();
        let explosion_target_ids = self
            .explosion_targets
            .into_iter()
            .map(|target| target.session_id)
            .collect::<HashSet<_>>();
        let mut recipient_ids = observer_ids
            .union(&explosion_target_ids)
            .copied()
            .collect::<Vec<_>>();
        recipient_ids.sort_unstable();

        let mut dispatches = Vec::new();
        for recipient_id in recipient_ids {
            let Some(session) = inner.sessions.get(&recipient_id) else {
                continue;
            };
            let recipient = ordered_session_recipient(recipient_id, session);
            if observer_ids.contains(&recipient_id) {
                dispatches.push(VisibilityDispatch {
                    recipient: recipient.clone(),
                    command: OutboundCommand::DespawnEntity(self.snapshot.clone()),
                });
            }
            if !explosion_target_ids.contains(&recipient_id) {
                continue;
            }
            let impact = impacts.get(&recipient_id).copied();
            if let Some(impact) = impact {
                dispatches.push(VisibilityDispatch {
                    recipient: recipient.clone(),
                    command: OutboundCommand::DamagePlayer {
                        damage: PlayerDamageRequest {
                            kind: PlayerDamageKind::Explosion,
                            amount: impact.damage,
                            source_origin: Some(self.position),
                        },
                    },
                });
            }
            dispatches.push(VisibilityDispatch {
                recipient,
                command: OutboundCommand::Explosion(tnt_explosion_packet(
                    self.position,
                    block_count,
                    impact.map(|impact| impact.knockback),
                )),
            });
        }
        record_entity_dispatches_locked(inner, &dispatches);
        dispatches
    }
}

impl SessionRegistry {
    pub(in crate::play) fn explosion_entity_targets(
        &self,
        _authority: &SimulationAuthority,
        center: Vec3,
        double_radius: f64,
    ) -> Vec<ExplosionEntityTarget> {
        if !center.is_finite() || !double_radius.is_finite() || double_radius <= 0.0 {
            return Vec::new();
        }

        let inner = self.lock_session_entities("snapshot explosion entity targets");
        let radius_sq = double_radius * double_radius;
        nearby_entity_candidate_ids_locked(&inner, center, double_radius)
            .into_iter()
            .filter_map(|entity_id| inner.entities.snapshot(entity_id))
            .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
            .filter(|entity| distance_sq(entity.position, center) <= radius_sq)
            .filter_map(|entity| {
                let facts = super::interaction_geometry::canonical_entity_facts(&entity.type_name)?;
                facts.attributes.max_health?;
                if entity.type_name == TNT_ENTITY_TYPE_NAME {
                    return None;
                }
                let geometry = entity_geometry(&entity.type_name, entity.animal);
                let aabb = geometry.aabb;
                Some(ExplosionEntityTarget {
                    entity_id: entity.id,
                    position: entity.position,
                    eye_position: Vec3::new(
                        entity.position.x,
                        entity.position.y + geometry.eye_height,
                        entity.position.z,
                    ),
                    aabb_min: Vec3::new(
                        entity.position.x - aabb.half_width,
                        entity.position.y,
                        entity.position.z - aabb.half_width,
                    ),
                    aabb_max: Vec3::new(
                        entity.position.x + aabb.half_width,
                        entity.position.y + aabb.height,
                        entity.position.z + aabb.half_width,
                    ),
                })
            })
            .collect()
    }

    pub(in crate::play) fn apply_explosion_entity_impacts(
        &self,
        _authority: &SimulationAuthority,
        impacts: &[ServerEntityExplosionImpact],
    ) -> Vec<VisibilityDispatch> {
        let mut dispatches = {
            let mut inner = self.lock_session_entities("apply explosion entity impacts");
            let mut dispatches = Vec::new();
            for impact in impacts {
                if !impact.damage.is_finite()
                    || impact.damage <= 0.0
                    || !impact.knockback.is_finite()
                {
                    continue;
                }
                let Some(target) = inner.entities.snapshot(impact.entity_id) else {
                    continue;
                };
                let rewards = EntityKillRewards {
                    items: inner.arrow_kill_rewards.item_entity_type_id.map_or_else(
                        Vec::new,
                        |entity_type_id| {
                            entity_kill_drop_stacks(
                                &inner.arrow_kill_rewards,
                                &target.type_name,
                                target.animal,
                                target.id.0 as i64 as u64,
                            )
                            .into_iter()
                            .map(|drop| (entity_type_id, entity_item_stack(drop)))
                            .collect()
                        },
                    ),
                    experience: inner
                        .arrow_kill_rewards
                        .xp_orb_entity_type_id
                        .map(|entity_type_id| (entity_type_id, mob_xp_value(&target.type_name))),
                };
                let Some(mut outcome) = attack_server_entity_locked(
                    &mut inner,
                    impact.entity_id,
                    impact.damage,
                    None,
                    &rewards,
                ) else {
                    continue;
                };
                if matches!(outcome, EntityAttackOutcome::Damaged { .. }) {
                    outcome
                        .dispatches_mut()
                        .extend(entity_event_dispatches_locked(&inner, impact.entity_id, 2));
                    let knockback = apply_explosion_knockback_locked(
                        &mut inner,
                        impact.entity_id,
                        impact.knockback,
                    );
                    outcome.dispatches_mut().extend(knockback);
                }
                dispatches.append(outcome.dispatches_mut());
            }
            dispatches
        };
        self.append_spawned_xp_pickup_candidates(&mut dispatches);
        dispatches
    }

    pub(in crate::play) fn claim_due_primed_tnt(
        &self,
        _authority: &SimulationAuthority,
        current_tick: u64,
    ) -> Vec<ExpiredPrimedTnt> {
        let mut inner = self.lock_session_entities("claim due primed TNT");
        let mut due_ids = inner
            .entities
            .snapshots_vec()
            .into_iter()
            .filter(|entity| {
                entity
                    .retained
                    .primed_tnt
                    .is_some_and(|fuse| fuse.expires_tick <= current_tick)
            })
            .map(|entity| entity.id)
            .collect::<Vec<_>>();
        due_ids.sort_unstable();

        let claimed = due_ids
            .into_iter()
            .filter_map(|entity_id| {
                let retained = inner.entities.snapshot(entity_id)?.retained.primed_tnt?;
                let snapshot = remove_server_entity_state_locked(&mut inner, entity_id)?;
                let observer_ids = remove_entity_visibility_locked(&mut inner, entity_id);
                let center = Vec3::new(
                    snapshot.position.x,
                    snapshot.position.y + 0.06125,
                    snapshot.position.z,
                );
                let explosion_targets = inner
                    .sessions
                    .iter()
                    .filter(|(_, session)| {
                        distance_sq(player_collision_position(session.pose), center) < 4096.0
                    })
                    .map(|(&id, session)| ExplosionPlayerTarget {
                        session_id: id,
                        pose: session.pose,
                    })
                    .collect();
                Some(ExpiredPrimedTnt {
                    entity_id,
                    position: snapshot.position,
                    entity_type_id: snapshot.type_id,
                    air: mc_world::BlockStateId(retained.air_block_state),
                    snapshot,
                    observer_ids,
                    explosion_targets,
                })
            })
            .collect::<Vec<_>>();
        drop(inner);
        claimed
    }

    pub(in crate::play) fn plan_expired_tnt_dispatches(
        &self,
        tnt: ExpiredPrimedTnt,
        block_count: i32,
        impacts: &HashMap<SessionId, PlayerExplosionImpact>,
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_inner("plan expired TNT publication");
        tnt.plan_dispatches(&mut inner, block_count, impacts)
    }

    #[cfg(test)]
    pub(in crate::play) fn primed_tnt_fuses_for_test(&self) -> Vec<(EntityId, u64)> {
        let inner = self.lock_session_entities("inspect primed TNT fuses");
        let mut fuses = inner
            .entities
            .snapshots_vec()
            .into_iter()
            .filter_map(|entity| {
                entity
                    .retained
                    .primed_tnt
                    .map(|fuse| (entity.id, fuse.expires_tick))
            })
            .collect::<Vec<_>>();
        fuses.sort_unstable_by_key(|fuse| fuse.0);
        fuses
    }

    pub(in crate::play) fn spawn_chained_primed_tnt(
        &self,
        _authority: &SimulationAuthority,
        entity_type_id: i32,
        position: Vec3,
        velocity: Vec3,
        fuse_ticks: u64,
        air: BlockStateId,
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_session_entities("spawn chained primed TNT");
        spawn_primed_tnt_locked(
            &mut inner,
            entity_type_id,
            position,
            velocity,
            fuse_ticks,
            air,
        )
        .1
    }
}

pub(super) fn spawn_primed_tnt_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_type_id: i32,
    position: Vec3,
    velocity: Vec3,
    fuse_ticks: u64,
    air: BlockStateId,
) -> (EntityId, Vec<VisibilityDispatch>) {
    let mut entity = SpawnEntity::new(entity_type_id, TNT_ENTITY_TYPE_NAME, position);
    entity.velocity = velocity;
    entity.on_ground = false;
    entity.retained.spawn_tick = inner.entity_lifecycle_tick;
    entity.retained.primed_tnt = Some(mc_entity::EntityPrimedTntState {
        expires_tick: inner.entity_lifecycle_tick.saturating_add(fuse_ticks),
        air_block_state: air.0,
    });
    let entity_id = inner.entities.spawn(entity);
    inner
        .entity_type_aabbs
        .entry(entity_type_id)
        .or_insert_with(|| entity_aabb(TNT_ENTITY_TYPE_NAME));
    track_entity_chunk_locked(inner, entity_id, position);
    initialize_entity_wire_state_locked(inner, entity_id);
    let dispatches = spawn_entity_visibility_locked(inner, entity_id);
    (entity_id, dispatches)
}

fn apply_explosion_knockback_locked(
    inner: &mut SessionEntityGuards<'_>,
    target_id: EntityId,
    knockback: Vec3,
) -> Vec<VisibilityDispatch> {
    if knockback == Vec3::ZERO {
        return Vec::new();
    }
    let Some(target) = inner.entities.snapshot(target_id) else {
        return Vec::new();
    };
    let velocity = Vec3::new(
        target.velocity.x + knockback.x,
        target.velocity.y + knockback.y,
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
                    wire_move: None,
                    velocity: snapshot.velocity,
                    rotation: snapshot.rotation,
                    on_ground: snapshot.on_ground,
                    send_velocity: true,
                    send_head_rotation: false,
                }),
            })
        })
        .collect()
}
