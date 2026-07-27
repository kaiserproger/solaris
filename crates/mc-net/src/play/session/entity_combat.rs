use super::entity_lifecycle::{nearby_entity_candidate_ids_locked, schedule_entity_death_locked};
use super::interaction_geometry::{entity_geometry, within_entity_attack_reach};
use super::player_combat::held_attack_range;
use super::player_state::{apply_player_survival_plan_locked, player_attack_cost_plan_matches};
use super::script_commit_events::push_player_entity_killed_event_locked;
use super::{
    CommittedPlayerAttackCosts, ENTITY_DEATH_TICKS, ENTITY_EVENT_DEATH,
    ENTITY_HURT_INVULNERABLE_TICKS, EntityAttackOutcome, EntityKillRewards, EntityStoreGuard,
    OutboundCommand, PlayerAttackResult, ServerEntitySnapshot, SessionEntityGuards, SessionId,
    SessionRegistry, VisibilityDispatch, apply_player_melee_knockback_locked,
    entity_event_dispatches_locked, entity_item_stack, entity_kill_drop_stacks, mob_xp_value,
    record_entity_dispatches_locked, server_entity_snapshot_from, session_recipients,
    spawn_item_drop_locked, spawn_xp_orb_locked, visibility_dispatches,
    visible_entity_observers_locked,
};
use crate::play::simulation::{PlayerSurvivalPlan, SimulationAuthority};
use crate::play::{GameMode, PlayerPose};
use mc_entity::{
    AttributeKind, EntityDamageRequest, EntityEffectRejection, EntityEffectRequest,
    EntityEffectResult, EntityId, EntityLifecycle, EntitySnapshot, Vec3,
};
use std::time::Instant;

const VILLAGER_WITNESS_FOLLOW_RANGE_DEFAULT: f64 = 16.0;
const VILLAGER_WITNESS_FOLLOW_RANGE_MAX: f64 = 2_048.0;

fn villager_killed_witness_snapshots_locked(
    inner: &SessionEntityGuards<'_>,
    victim: &EntitySnapshot,
) -> Vec<EntitySnapshot> {
    if victim.type_name != "minecraft:villager" || victim.retained.villager.is_none() {
        return Vec::new();
    }
    let follow_range = victim
        .attributes
        .base(&AttributeKind::FollowRange)
        .unwrap_or(VILLAGER_WITNESS_FOLLOW_RANGE_DEFAULT);
    if !follow_range.is_finite() || follow_range <= 0.0 {
        return Vec::new();
    }
    let follow_range = follow_range.min(VILLAGER_WITNESS_FOLLOW_RANGE_MAX);
    nearby_entity_candidate_ids_locked(inner, victim.position, follow_range)
        .into_iter()
        .filter(|id| *id != victim.id)
        .filter_map(|id| inner.entities.snapshot(id))
        .filter(|witness| {
            witness.lifecycle == EntityLifecycle::Alive
                && witness.type_name == "minecraft:villager"
                && witness.retained.villager.is_some()
                && (witness.position.x - victim.position.x).abs() <= follow_range
                && (witness.position.y - victim.position.y).abs() <= follow_range
                && (witness.position.z - victim.position.z).abs() <= follow_range
        })
        .collect()
}

pub(super) fn commit_villager_killed_witness_gossip(
    entities: &mut EntityStoreGuard<'_>,
    witnesses: Vec<EntitySnapshot>,
    murderer: uuid::Uuid,
) -> usize {
    let transitions = witnesses
        .into_iter()
        .filter_map(|expected| {
            if expected.lifecycle != EntityLifecycle::Alive
                || expected.type_name != "minecraft:villager"
                || expected.retained.villager.is_none()
            {
                return None;
            }
            let mut next = expected.clone();
            let gossip = next
                .retained
                .villager_gossip
                .get_or_insert_with(Default::default);
            gossip
                .record_event(
                    mc_entity::villager_gossip_26_1_2::VillagerGossipEvent::KilledByPlayer {
                        player: murderer,
                    },
                )
                .then_some((expected, next))
        })
        .collect::<Vec<_>>();
    commit_villager_killed_witness_gossip_batch(entities, transitions)
}

fn commit_villager_killed_witness_gossip_batch(
    entities: &mut EntityStoreGuard<'_>,
    mut batch: Vec<(EntitySnapshot, EntitySnapshot)>,
) -> usize {
    let count = batch.len();
    if count == 0 {
        return 0;
    }
    if entities.replace_snapshots_if_current(batch.iter().cloned()) {
        return count;
    }
    if count == 1 {
        return 0;
    }
    let right = batch.split_off(count / 2);
    commit_villager_killed_witness_gossip_batch(entities, batch)
        + commit_villager_killed_witness_gossip_batch(entities, right)
}

pub(in crate::play) struct ServerEntityPlayerAttack<'a> {
    pub(in crate::play) entity_id: EntityId,
    pub(in crate::play) amount: f32,
    pub(in crate::play) game_mode: GameMode,
    pub(in crate::play) player_pose: PlayerPose,
    pub(in crate::play) attacker: Option<(SessionId, &'a PlayerSurvivalPlan)>,
}

impl SessionRegistry {
    pub(in crate::play) fn apply_server_entity_effect_request(
        &self,
        _authority: &SimulationAuthority,
        expected: Option<EntitySnapshot>,
        entity_id: EntityId,
        request: EntityEffectRequest,
    ) -> (EntityEffectResult, Vec<VisibilityDispatch>) {
        let mut inner = self.lock_session_entities("apply server entity effect transaction");
        let Some(expected) = expected.or_else(|| inner.entities.snapshot(entity_id)) else {
            return (
                EntityEffectResult::Rejected(EntityEffectRejection::Missing),
                Vec::new(),
            );
        };
        apply_server_entity_effect_request_locked(&mut inner, expected, request)
    }

    pub(super) fn player_attack_server_entity(
        &self,
        _authority: &SimulationAuthority,
        attack: ServerEntityPlayerAttack<'_>,
    ) -> PlayerAttackResult {
        let ServerEntityPlayerAttack {
            entity_id,
            amount,
            game_mode,
            player_pose,
            attacker,
        } = attack;
        if game_mode == GameMode::Spectator {
            return PlayerAttackResult::ValidationRejected;
        }
        let mut inner = self.lock_session_entities("player attack server entity");
        let Some(target) = inner.entities.snapshot(entity_id) else {
            return PlayerAttackResult::ValidationRejected;
        };
        if target.item_stack.is_some() {
            return PlayerAttackResult::ValidationRejected;
        }
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
        let knockback_origin = (game_mode == GameMode::Survival).then_some(Vec3::new(
            player_pose.x,
            player_pose.y,
            player_pose.z,
        ));
        let attacker_uuid = if let Some((attacker_session, _)) = attacker {
            let Some(session) = inner.sessions.get(&attacker_session) else {
                return PlayerAttackResult::ValidationRejected;
            };
            Some(session.uuid)
        } else {
            None
        };
        let attacker_persistence = if let Some((attacker_session, _)) = attacker {
            let Some(state) = inner.player_persistence.get(&attacker_session).cloned() else {
                return PlayerAttackResult::ValidationRejected;
            };
            Some(state)
        } else {
            None
        };
        let mut attacker_state = if let Some(((_, attacker_costs), state)) =
            attacker.zip(attacker_persistence.as_ref())
        {
            let wait_started = Instant::now();
            let state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let state = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::PlayerPersistence,
                "commit server-entity attack costs",
                wait_started,
                state,
            );
            if !player_attack_cost_plan_matches(&state, attacker_costs)
                || state.game_mode != game_mode
                || state.game_mode == GameMode::Spectator
                || state.survival.is_dead()
            {
                return PlayerAttackResult::ValidationRejected;
            }
            Some(state)
        } else {
            None
        };
        let attack_range = attacker_state
            .as_deref()
            .and_then(|state| held_attack_range(&inner.player_combat, state));
        if !within_entity_attack_reach(
            player_pose,
            target.position,
            entity_geometry(&target.type_name, target.animal).aabb,
            game_mode,
            attack_range,
        ) {
            return PlayerAttackResult::ValidationRejected;
        }
        let gossip_event =
            (target.type_name == "minecraft:villager")
                .then_some(attacker_uuid)
                .flatten()
                .map(|player| {
                    mc_entity::villager_gossip_26_1_2::VillagerGossipEvent::HurtByPlayer { player }
                });
        let Some(mut outcome) = attack_server_entity_locked(
            &mut inner,
            entity_id,
            amount,
            knockback_origin,
            &rewards,
            gossip_event,
        ) else {
            return PlayerAttackResult::AcceptedNoDamage;
        };
        let committed_attacker = attacker.zip(attacker_state.as_mut()).map(
            |((attacker_session, costs), attacker_state)| {
                let mut effective = costs.clone();
                effective.expected_survival = attacker_state.survival;
                effective.updated_survival.health = attacker_state.survival.health;
                let committed = apply_player_survival_plan_locked(
                    &mut inner,
                    attacker_session,
                    attacker_state,
                    &effective,
                );
                CommittedPlayerAttackCosts {
                    survival: committed.survival,
                    inventory: committed.inventory,
                }
            },
        );
        match &mut outcome {
            EntityAttackOutcome::Damaged { attacker_costs, .. }
            | EntityAttackOutcome::Killed { attacker_costs, .. } => {
                *attacker_costs = committed_attacker;
            }
            EntityAttackOutcome::PlayerDamaged { .. } => unreachable!("server entity outcome"),
        }
        if let (Some((attacker_session, _)), EntityAttackOutcome::Killed { entity, .. }) =
            (attacker, &outcome)
        {
            push_player_entity_killed_event_locked(
                &inner,
                attacker_session,
                game_mode,
                player_pose,
                entity,
            );
        }
        drop(inner);
        self.append_spawned_xp_pickup_candidates(outcome.dispatches_mut());
        PlayerAttackResult::Damaged(Box::new(outcome))
    }

    #[cfg(test)]
    pub(in crate::play) fn attack_server_entity(
        &self,
        _authority: &SimulationAuthority,
        entity_id: EntityId,
        amount: f32,
        knockback_origin: Option<Vec3>,
        rewards: &EntityKillRewards,
    ) -> Option<EntityAttackOutcome> {
        let mut outcome = {
            let mut inner = self.lock_session_entities("attack server entity");
            attack_server_entity_locked(
                &mut inner,
                entity_id,
                amount,
                knockback_origin,
                rewards,
                None,
            )?
        };
        self.append_spawned_xp_pickup_candidates(outcome.dispatches_mut());
        Some(outcome)
    }

    #[cfg(test)]
    pub(in crate::play) fn damage_server_entity_for_test(
        &self,
        entity_id: EntityId,
        amount: f32,
    ) -> Option<mc_entity::EntityDamage> {
        let mut inner = self.lock_session_entities("damage server entity test");
        let damage = damage_server_entity_locked(&mut inner, entity_id, amount, None)?;
        if damage.killed {
            schedule_entity_death_locked(&mut inner, &damage.snapshot);
        }
        Some(damage)
    }

    #[cfg(test)]
    pub(in crate::play) fn publish_entity_health_snapshot_for_test(
        &self,
        snapshot: EntitySnapshot,
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_session_entities("publish test entity health");
        publish_accepted_entity_health_locked(&mut inner, &snapshot)
    }

    #[cfg(test)]
    pub(in crate::play) fn published_entity_health_for_test(
        &self,
        entity_id: EntityId,
    ) -> Option<f32> {
        self.lock_inner("read test published entity health")
            .published_entity_snapshots
            .get(&entity_id)
            .and_then(|snapshot| snapshot.health)
    }
}

fn apply_server_entity_effect_request_locked(
    inner: &mut SessionEntityGuards<'_>,
    expected: EntitySnapshot,
    request: EntityEffectRequest,
) -> (EntityEffectResult, Vec<VisibilityDispatch>) {
    if server_entity_snapshot_from(expected.clone())
        .health
        .is_none()
    {
        return (
            EntityEffectResult::Rejected(EntityEffectRejection::NonLiving),
            Vec::new(),
        );
    }
    let result = inner.entities.apply_effect_if_current(expected, request);
    let dispatches = match &result {
        EntityEffectResult::Applied(applied) => {
            schedule_entity_death_locked(inner, &applied.snapshot);
            publish_accepted_entity_health_locked(inner, &applied.snapshot)
        }
        EntityEffectResult::Rejected(_) => Vec::new(),
    };
    (result, dispatches)
}

pub(super) fn publish_accepted_entity_health_locked(
    inner: &mut SessionEntityGuards<'_>,
    accepted: &EntitySnapshot,
) -> Vec<VisibilityDispatch> {
    let Some(current) = inner.entities.snapshot(accepted.id) else {
        return Vec::new();
    };
    if &current != accepted {
        return Vec::new();
    }
    let projected = server_entity_snapshot_from(current);
    let Some(health) = projected.health else {
        return Vec::new();
    };
    if inner
        .published_entity_snapshots
        .get(&projected.id)
        .is_some_and(|published| published.health == Some(health))
    {
        return Vec::new();
    }
    if let Some(published) = inner.published_entity_snapshots.get_mut(&projected.id) {
        published.health = Some(health);
    } else {
        inner
            .published_entity_snapshots
            .insert(projected.id, projected.clone());
    }
    let observer_ids = visible_entity_observers_locked(inner, projected.id);
    let recipients = session_recipients(inner, observer_ids);
    let dispatches = visibility_dispatches(recipients, || {
        OutboundCommand::UpdateEntityHealth(projected.clone())
    });
    record_entity_dispatches_locked(inner, &dispatches);
    dispatches
}

pub(super) fn begin_server_entity_death_locked(
    inner: &mut SessionEntityGuards<'_>,
    damage: &mc_entity::EntityDamage,
    rewards: &EntityKillRewards,
) -> (ServerEntitySnapshot, Vec<VisibilityDispatch>) {
    let entity_id = damage.snapshot.id;
    schedule_entity_death_locked(inner, &damage.snapshot);
    let entity = server_entity_snapshot_from(damage.snapshot.clone());
    let mut dispatches = Vec::new();
    for (entity_type_id, stack) in &rewards.items {
        dispatches.extend(spawn_item_drop_locked(
            inner,
            *entity_type_id,
            entity.position,
            stack.clone(),
        ));
    }
    if let Some((entity_type_id, value)) = rewards.experience {
        dispatches.extend(spawn_xp_orb_locked(
            inner,
            entity_type_id,
            entity.position,
            value,
        ));
    }
    dispatches.extend(entity_event_dispatches_locked(
        inner,
        entity_id,
        ENTITY_EVENT_DEATH,
    ));
    (entity, dispatches)
}

pub(super) fn attack_server_entity_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_id: EntityId,
    amount: f32,
    knockback_origin: Option<Vec3>,
    rewards: &EntityKillRewards,
    gossip_event: Option<mc_entity::villager_gossip_26_1_2::VillagerGossipEvent>,
) -> Option<EntityAttackOutcome> {
    let expected = inner.entities.snapshot(entity_id)?;
    let murderer = match gossip_event {
        Some(mc_entity::villager_gossip_26_1_2::VillagerGossipEvent::HurtByPlayer { player }) => {
            Some(player)
        }
        _ => None,
    };
    let witnesses = if murderer.is_some()
        && amount.is_finite()
        && amount >= expected.health
        && expected.lifecycle == EntityLifecycle::Alive
    {
        villager_killed_witness_snapshots_locked(inner, &expected)
    } else {
        Vec::new()
    };
    let damage = damage_expected_server_entity_locked(inner, expected, amount, gossip_event)?;
    let health_dispatches = publish_accepted_entity_health_locked(inner, &damage.snapshot);
    if damage.killed {
        if let Some(murderer) = murderer {
            commit_villager_killed_witness_gossip(&mut inner.entities, witnesses, murderer);
        }
        let (entity, mut dispatches) = begin_server_entity_death_locked(inner, &damage, rewards);
        dispatches.splice(0..0, health_dispatches);
        return Some(EntityAttackOutcome::Killed {
            damage,
            entity,
            dispatches,
            attacker_costs: None,
        });
    }
    let mut dispatches = health_dispatches;
    dispatches.extend(knockback_origin.map_or_else(Vec::new, |origin| {
        apply_player_melee_knockback_locked(inner, entity_id, origin)
    }));
    Some(EntityAttackOutcome::Damaged {
        damage,
        dispatches,
        attacker_costs: None,
    })
}

#[cfg(test)]
pub(super) fn damage_server_entity_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_id: EntityId,
    amount: f32,
    gossip_event: Option<mc_entity::villager_gossip_26_1_2::VillagerGossipEvent>,
) -> Option<mc_entity::EntityDamage> {
    let expected = inner.entities.snapshot(entity_id)?;
    damage_expected_server_entity_locked(inner, expected, amount, gossip_event)
}

fn damage_expected_server_entity_locked(
    inner: &mut SessionEntityGuards<'_>,
    expected: EntitySnapshot,
    amount: f32,
    gossip_event: Option<mc_entity::villager_gossip_26_1_2::VillagerGossipEvent>,
) -> Option<mc_entity::EntityDamage> {
    let tick = inner.entity_lifecycle_tick;
    if expected
        .retained
        .last_damage_tick
        .is_some_and(|last| tick.saturating_sub(last) < ENTITY_HURT_INVULNERABLE_TICKS)
    {
        return None;
    }
    inner.entities.damage_if_current(
        expected,
        EntityDamageRequest {
            amount,
            tick,
            death_remove_tick: tick.saturating_add(ENTITY_DEATH_TICKS),
            villager_gossip_event: gossip_event,
        },
    )
}
