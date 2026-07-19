use super::interaction_geometry::{entity_geometry, within_entity_reach};
use super::player_state::{apply_player_survival_plan_locked, player_attack_cost_plan_matches};
use super::{
    CommittedPlayerAttackCosts, ENTITY_DEATH_TICKS, ENTITY_EVENT_DEATH,
    ENTITY_HURT_INVULNERABLE_TICKS, EntityAttackOutcome, EntityKillRewards, PlayerAttackResult,
    ServerEntitySnapshot, SessionEntityGuards, SessionId, SessionRegistry, VisibilityDispatch,
    apply_player_melee_knockback_locked, entity_event_dispatches_locked, entity_item_stack,
    entity_kill_drop_stacks, mob_xp_value, server_entity_snapshot_from, spawn_item_drop_locked,
    spawn_xp_orb_locked,
};
use crate::play::simulation::{PlayerSurvivalPlan, SimulationAuthority};
use crate::play::{GameMode, PlayerPose};
use mc_entity::{EntityId, Vec3};
use std::time::Instant;

pub(in crate::play) struct ServerEntityPlayerAttack<'a> {
    pub(in crate::play) entity_id: EntityId,
    pub(in crate::play) amount: f32,
    pub(in crate::play) game_mode: GameMode,
    pub(in crate::play) player_pose: PlayerPose,
    pub(in crate::play) attacker: Option<(SessionId, &'a PlayerSurvivalPlan)>,
}

impl SessionRegistry {
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
        if !within_entity_reach(
            player_pose,
            target.position,
            entity_geometry(&target.type_name, target.animal).aabb,
            game_mode,
        ) {
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
        let Some(mut outcome) =
            attack_server_entity_locked(&mut inner, entity_id, amount, knockback_origin, &rewards)
        else {
            return PlayerAttackResult::AcceptedNoDamage;
        };
        let committed_attacker =
            attacker
                .zip(attacker_state.as_mut())
                .map(|((_, costs), attacker_state)| {
                    let mut effective = costs.clone();
                    effective.expected_survival = attacker_state.survival;
                    effective.updated_survival.health = attacker_state.survival.health;
                    let committed =
                        apply_player_survival_plan_locked(&mut inner, attacker_state, &effective);
                    CommittedPlayerAttackCosts {
                        survival: committed.survival,
                        inventory: committed.inventory,
                    }
                });
        match &mut outcome {
            EntityAttackOutcome::Damaged { attacker_costs, .. }
            | EntityAttackOutcome::Killed { attacker_costs, .. } => {
                *attacker_costs = committed_attacker;
            }
            EntityAttackOutcome::PlayerDamaged { .. } => unreachable!("server entity outcome"),
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
            attack_server_entity_locked(&mut inner, entity_id, amount, knockback_origin, rewards)?
        };
        self.append_spawned_xp_pickup_candidates(outcome.dispatches_mut());
        Some(outcome)
    }

    #[cfg(test)]
    pub(in crate::play) fn damage_server_entity_legacy_for_test(
        &self,
        entity_id: EntityId,
        amount: f32,
    ) -> Option<mc_entity::EntityDamage> {
        let mut inner = self.lock_session_entities("damage server entity legacy test");
        damage_server_entity_locked(&mut inner, entity_id, amount)
    }
}

pub(super) fn begin_server_entity_death_locked(
    inner: &mut SessionEntityGuards<'_>,
    damage: &mc_entity::EntityDamage,
    rewards: &EntityKillRewards,
) -> (ServerEntitySnapshot, Vec<VisibilityDispatch>) {
    let entity_id = damage.snapshot.id;
    let remove_tick = inner
        .entity_lifecycle_tick
        .saturating_add(ENTITY_DEATH_TICKS);
    inner
        .dying_entity_remove_ticks
        .insert(entity_id, remove_tick);
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
) -> Option<EntityAttackOutcome> {
    let damage = damage_server_entity_locked(inner, entity_id, amount)?;
    if damage.killed {
        let (entity, dispatches) = begin_server_entity_death_locked(inner, &damage, rewards);
        return Some(EntityAttackOutcome::Killed {
            damage,
            entity,
            dispatches,
            attacker_costs: None,
        });
    }
    let dispatches = knockback_origin.map_or_else(Vec::new, |origin| {
        apply_player_melee_knockback_locked(inner, entity_id, origin)
    });
    Some(EntityAttackOutcome::Damaged {
        damage,
        dispatches,
        attacker_costs: None,
    })
}

pub(super) fn damage_server_entity_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_id: EntityId,
    amount: f32,
) -> Option<mc_entity::EntityDamage> {
    let tick = inner.entity_lifecycle_tick;
    if inner
        .last_entity_damage_ticks
        .get(&entity_id)
        .is_some_and(|last| tick.saturating_sub(*last) < ENTITY_HURT_INVULNERABLE_TICKS)
    {
        return None;
    }
    let damage = inner.entities.damage(entity_id, amount)?;
    inner.last_entity_damage_ticks.insert(entity_id, tick);
    Some(damage)
}
