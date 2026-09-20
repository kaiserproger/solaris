use super::damage_precommit::{
    EntityDamagePrecommitCompletion, EntityDamagePrecommitResult, EntityDamagePrecommitResume,
    begin_entity_damage,
};
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
    entity_event_dispatches_locked, entity_hurt_dispatches_locked, entity_item_stack,
    entity_kill_drop_stacks, mob_xp_value, record_entity_dispatches_locked,
    server_entity_snapshot_from, session_recipients, spawn_item_drop_locked, spawn_xp_orb_locked,
    visibility_dispatches, visible_entity_observers_locked,
};
use crate::lock_policy::lock_authoritative_mutex;
use crate::play::simulation::{
    PlayerSurvivalPlan, SimulationAuthority, SimulationCommand, SimulationRequestError,
    SimulationResponseSender,
};
use crate::play::{GameMode, PlayerPose};
use mc_entity::dragon_26_1_2::{
    DragonAirPhase, DragonAirState, DragonPart, dragon_part_damage, part_center,
};
use mc_entity::{
    AttributeKind, EntityDamageRequest, EntityEffectOperation, EntityEffectRejection,
    EntityEffectRequest, EntityEffectResult, EntityId, EntityLifecycle, EntitySnapshot, Vec3,
};
use mc_physics::Aabb;
use mc_script::precommit::{HookActor, HookKind, HookPlayer};
use std::time::Instant;

const VILLAGER_WITNESS_FOLLOW_RANGE_DEFAULT: f64 = 16.0;
const VILLAGER_WITNESS_FOLLOW_RANGE_MAX: f64 = 2_048.0;

struct ResolvedServerAttackTarget {
    snapshot: EntitySnapshot,
    reach_position: Vec3,
    reach_aabb: Aabb,
    dragon_part: Option<DragonPart>,
}

pub(super) struct DeferredEntityDamage {
    expected: EntitySnapshot,
    request: EntityDamageRequest,
    source: HookActor,
    kind: &'static str,
    position: Vec3,
    attacker_costs: Option<(SessionId, PlayerSurvivalPlan)>,
    response: Option<SimulationResponseSender>,
    completion: EntityDamagePrecommitCompletion,
}

/// Ask the shared damage boundary after a producer has released every native
/// owner guard.  A `true` result is terminal for that producer's current turn:
/// the detached continuation owns the frozen CAS image and is the only path
/// allowed to commit it after the guest answers.
pub(super) fn defer_entity_damage_precommit(
    sessions: &SessionRegistry,
    deferred: DeferredEntityDamage,
) -> bool {
    let DeferredEntityDamage {
        expected,
        request,
        source,
        kind,
        position,
        attacker_costs,
        response,
        completion,
    } = deferred;
    if !sessions
        .precommit_boundary()
        .is_some_and(|boundary| boundary.has_precommit_hooks(HookKind::Damage))
    {
        return false;
    }
    let Some(handle) = sessions.damage_precommit_handle().cloned() else {
        if let Some(response) = response {
            let _ = response.send(Err(SimulationRequestError::Precommit(
                mc_script::precommit::HookFailure::Unavailable,
            )));
        }
        return true;
    };
    let pending = match begin_entity_damage(
        sessions,
        expected.id,
        source,
        kind,
        position,
        request.amount,
    ) {
        Ok(Some(pending)) => pending,
        Ok(None) => return false,
        Err(error) => {
            if let Some(response) = response {
                let _ = response.send(Err(SimulationRequestError::Precommit(error)));
            }
            return true;
        }
    };
    handle.spawn_precommit_resume(pending, None, response, move |decision| {
        SimulationCommand::ResumeEntityDamagePrecommit(Box::new(EntityDamagePrecommitResume {
            expected,
            request,
            attacker_costs,
            completion,
            decision,
        }))
    });
    true
}

fn entity_effect_damage_amount(operation: &EntityEffectOperation) -> Option<(f32, &'static str)> {
    let EntityEffectOperation::ApplyAction { action, .. } = operation else {
        return None;
    };
    match action {
        mc_entity::effects_26_1_2::EffectAction::Damage { amount, .. } => Some((*amount, "effect")),
        mc_entity::effects_26_1_2::EffectAction::MagicDamageIfHealthAbove { amount, .. } => {
            Some((*amount, "magic"))
        }

        _ => None,
    }
}
fn replace_entity_effect_damage_amount(request: &mut EntityEffectRequest, amount: f32) -> bool {
    let EntityEffectOperation::ApplyAction { action, .. } = &mut request.operation else {
        return false;
    };
    match action {
        mc_entity::effects_26_1_2::EffectAction::Damage {
            amount: raw_amount, ..
        }
        | mc_entity::effects_26_1_2::EffectAction::MagicDamageIfHealthAbove {
            amount: raw_amount,
            ..
        } => {
            *raw_amount = amount;
            true
        }
        _ => false,
    }
}

fn entity_damage_precommit_refusal(
    completion: &EntityDamagePrecommitCompletion,
) -> EntityDamagePrecommitResult {
    match completion {
        EntityDamagePrecommitCompletion::Direct { .. } => EntityDamagePrecommitResult::Direct,
        EntityDamagePrecommitCompletion::Script => EntityDamagePrecommitResult::Script(None),
        EntityDamagePrecommitCompletion::Resident { .. } => {
            EntityDamagePrecommitResult::Resident(None)
        }
        EntityDamagePrecommitCompletion::Effect { .. } => EntityDamagePrecommitResult::Effect(
            EntityEffectResult::Rejected(EntityEffectRejection::Stale),
        ),
        EntityDamagePrecommitCompletion::Projectile(_)
        | EntityDamagePrecommitCompletion::Explosion(_)
        | EntityDamagePrecommitCompletion::Golem(_) => EntityDamagePrecommitResult::Projectile,
    }
}

fn dragon_part_reach(snapshot: &EntitySnapshot, part: DragonPart) -> Option<(Vec3, Aabb)> {
    let state = snapshot
        .retained
        .dragon_air
        .unwrap_or_else(|| DragonAirState::new(snapshot.position, snapshot.rotation.yaw));
    let position = part_center(&state, snapshot.position, snapshot.rotation.yaw, part)?;
    let dimensions = part.dimensions();
    Some((
        position,
        Aabb {
            half_width: dimensions.width * 0.5,
            height: dimensions.height,
        },
    ))
}

fn resolve_server_attack_target_locked(
    inner: &SessionEntityGuards<'_>,
    requested_id: EntityId,
) -> Option<ResolvedServerAttackTarget> {
    if let Some(snapshot) = inner.entities.snapshot(requested_id) {
        if snapshot.type_name == "minecraft:ender_dragon" {
            let (reach_position, reach_aabb) = dragon_part_reach(&snapshot, DragonPart::Body)?;
            return Some(ResolvedServerAttackTarget {
                snapshot,
                reach_position,
                reach_aabb,
                dragon_part: Some(DragonPart::Body),
            });
        }
        let reach_aabb = entity_geometry(&snapshot.type_name, snapshot.animal).aabb;
        return Some(ResolvedServerAttackTarget {
            reach_position: snapshot.position,
            reach_aabb,
            snapshot,
            dragon_part: None,
        });
    }

    let mut dragon_part = None;
    inner.entities.visit_simulation_entities(|entity| {
        if dragon_part.is_some()
            || entity.lifecycle != EntityLifecycle::Alive
            || entity.type_name != "minecraft:ender_dragon"
        {
            return;
        }
        let Some(offset) = requested_id.0.checked_sub(entity.id.0) else {
            return;
        };
        if let Some(part) = DragonPart::from_protocol_offset(offset) {
            dragon_part = Some((entity.id, part));
        }
    });
    let (dragon_id, part) = dragon_part?;
    let snapshot = inner.entities.snapshot(dragon_id)?;
    let (reach_position, reach_aabb) = dragon_part_reach(&snapshot, part)?;
    Some(ResolvedServerAttackTarget {
        snapshot,
        reach_position,
        reach_aabb,
        dragon_part: Some(part),
    })
}

pub(super) fn entity_kill_rewards_locked(
    inner: &SessionEntityGuards<'_>,
    target: &EntitySnapshot,
) -> EntityKillRewards {
    EntityKillRewards {
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
    }
}

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

fn attack_dragon_part_locked(
    inner: &mut SessionEntityGuards<'_>,
    expected: EntitySnapshot,
    part: DragonPart,
    amount: f32,
) -> Option<EntityAttackOutcome> {
    let mut state = expected
        .retained
        .dragon_air
        .unwrap_or_else(|| DragonAirState::new(expected.position, expected.rotation.yaw));
    let resolved = dragon_part_damage(state.phase, part, amount, true)?;
    let tick = inner.entity_lifecycle_tick;
    if expected
        .retained
        .last_damage_tick
        .is_some_and(|last| tick.saturating_sub(last) < ENTITY_HURT_INVULNERABLE_TICKS)
    {
        return None;
    }

    let lethal = resolved >= expected.health;
    let mut next = expected.clone();
    next.health = if lethal {
        1.0
    } else {
        (expected.health - resolved).max(0.0)
    };
    next.retained.last_damage_tick = Some(tick);
    next.retained.living.invulnerable_time =
        u32::try_from(ENTITY_HURT_INVULNERABLE_TICKS).unwrap_or(u32::MAX);
    next.retained.living.hurt_time =
        u32::try_from(ENTITY_HURT_INVULNERABLE_TICKS).unwrap_or(u32::MAX);
    next.retained.living.last_hurt = resolved;
    if lethal {
        state.phase = DragonAirPhase::Dying;
        state.death_time = 0;
        state.fly_target = None;
        state.clear_target();
        next.velocity = Vec3::new(0.0, 0.1, 0.0);
        next.goal = mc_entity::GoalState::Idle;
    }
    next.retained.dragon_air = Some(state);
    if !inner
        .entities
        .replace_snapshot_if_current(expected, next.clone())
    {
        return None;
    }

    let damage = mc_entity::EntityDamage {
        snapshot: next,
        killed: false,
    };
    let mut dispatches = publish_accepted_entity_health_locked(inner, &damage.snapshot);
    dispatches.extend(entity_hurt_dispatches_locked(inner, damage.snapshot.id));
    Some(EntityAttackOutcome::Damaged {
        damage,
        dispatches,
        attacker_costs: None,
    })
}

impl SessionRegistry {
    pub(in crate::play) fn apply_server_entity_effect_request(
        &self,
        _authority: &SimulationAuthority,
        expected: Option<EntitySnapshot>,
        entity_id: EntityId,
        request: EntityEffectRequest,
        response: &mut Option<SimulationResponseSender>,
    ) -> (EntityEffectResult, Vec<VisibilityDispatch>) {
        let mut inner = self.lock_session_entities("apply server entity effect transaction");
        let Some(expected) = expected.or_else(|| inner.entities.snapshot(entity_id)) else {
            return (
                EntityEffectResult::Rejected(EntityEffectRejection::Missing),
                Vec::new(),
            );
        };
        let damage = entity_effect_damage_amount(&request.operation);
        if let Some((amount, kind)) = damage
            && self
                .precommit_boundary()
                .is_some_and(|boundary| boundary.has_precommit_hooks(HookKind::Damage))
        {
            let raw = EntityDamageRequest {
                amount,
                tick: inner.entity_lifecycle_tick,
                death_remove_tick: request.death_remove_tick,
                villager_gossip_event: None,
            };
            let position = expected.position;
            drop(inner);
            if defer_entity_damage_precommit(
                self,
                DeferredEntityDamage {
                    expected,
                    request: raw,
                    source: HookActor::Environment,
                    kind,
                    position,
                    attacker_costs: None,
                    response: response.take(),
                    completion: EntityDamagePrecommitCompletion::Effect {
                        request: request.clone(),
                    },
                },
            ) {
                return (
                    EntityEffectResult::Rejected(EntityEffectRejection::Stale),
                    Vec::new(),
                );
            }
            unreachable!("a registered damage hook must either defer or refuse an effect");
        }
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
        let Some(ResolvedServerAttackTarget {
            snapshot: target,
            reach_position,
            reach_aabb,
            dragon_part,
        }) = resolve_server_attack_target_locked(&inner, entity_id)
        else {
            return PlayerAttackResult::ValidationRejected;
        };
        if target.item_stack.is_some() {
            return PlayerAttackResult::ValidationRejected;
        }
        let rewards = entity_kill_rewards_locked(&inner, &target);
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
            let state = lock_authoritative_mutex(state, "play.player_persistence");
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
            reach_position,
            reach_aabb,
            game_mode,
            attack_range,
        ) {
            return PlayerAttackResult::ValidationRejected;
        }
        if self
            .precommit_boundary()
            .is_some_and(|boundary| boundary.has_precommit_hooks(HookKind::Damage))
        {
            let request = EntityDamageRequest {
                amount,
                tick: inner.entity_lifecycle_tick,
                death_remove_tick: inner
                    .entity_lifecycle_tick
                    .saturating_add(ENTITY_DEATH_TICKS),
                villager_gossip_event: None,
            };
            let source = attacker
                .zip(attacker_uuid)
                .and_then(|((session, _), uuid)| HookPlayer::try_new(uuid, session).ok())
                .map(HookActor::Player)
                .unwrap_or(HookActor::Environment);
            let deferred_costs = attacker.map(|(session, costs)| (session, costs.clone()));
            let deferred_target = target.clone();
            let attacker_session = attacker.map(|(session, _)| session);
            drop(attacker_state);
            drop(inner);
            if defer_entity_damage_precommit(
                self,
                DeferredEntityDamage {
                    expected: deferred_target.clone(),
                    request,
                    source,
                    kind: "player-attack",
                    position: deferred_target.position,
                    attacker_costs: deferred_costs,
                    response: None,
                    completion: EntityDamagePrecommitCompletion::Direct {
                        entity_id,
                        game_mode,
                        player_pose,
                        attacker_session,
                        dragon_part,
                    },
                },
            ) {
                return PlayerAttackResult::AcceptedNoDamage;
            }
            unreachable!("a registered damage hook must either defer or refuse the attack");
        }
        let gossip_event =
            (target.type_name == "minecraft:villager")
                .then_some(attacker_uuid)
                .flatten()
                .map(|player| {
                    mc_entity::villager_gossip_26_1_2::VillagerGossipEvent::HurtByPlayer { player }
                });
        let outcome = if let Some(part) = dragon_part {
            attack_dragon_part_locked(&mut inner, target, part, amount)
        } else {
            attack_server_entity_locked(
                &mut inner,
                target.id,
                amount,
                knockback_origin,
                &rewards,
                gossip_event,
            )
        };
        let Some(mut outcome) = outcome else {
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
    pub(in crate::play) fn resume_entity_damage_precommit(
        &self,
        _authority: &SimulationAuthority,
        resume: EntityDamagePrecommitResume,
    ) -> (EntityDamagePrecommitResult, Vec<VisibilityDispatch>) {
        let EntityDamagePrecommitResume {
            expected,
            mut request,
            attacker_costs,
            completion,
            decision,
        } = resume;
        if let EntityDamagePrecommitCompletion::Projectile(continuation) = completion {
            let dispatches = super::projectiles::resume_projectile_entity_damage_precommit(
                self,
                continuation,
                request,
                decision,
            );
            return (EntityDamagePrecommitResult::Projectile, dispatches);
        }
        if let EntityDamagePrecommitCompletion::Explosion(continuation) = completion {
            let dispatches = super::explosion_authority::resume_explosion_entity_damage_precommit(
                self,
                expected,
                request,
                decision,
                continuation,
            );
            return (EntityDamagePrecommitResult::Projectile, dispatches);
        }
        if let EntityDamagePrecommitCompletion::Golem(continuation) = completion {
            let dispatches = super::village_defense::resume_golem_entity_damage_precommit(
                self,
                expected,
                request,
                decision,
                continuation,
            );
            return (EntityDamagePrecommitResult::Projectile, dispatches);
        }
        let Ok(mut approval) = decision else {
            return (entity_damage_precommit_refusal(&completion), Vec::new());
        };
        request.amount = match approval.decision() {
            mc_script::precommit::HookDecision::Keep => request.amount,
            mc_script::precommit::HookDecision::Replace(amount)
                if amount.is_finite() && amount > 0.0 =>
            {
                amount
            }
            mc_script::precommit::HookDecision::Cancel
            | mc_script::precommit::HookDecision::Replace(_)
            | _ => {
                return (entity_damage_precommit_refusal(&completion), Vec::new());
            }
        };
        if let EntityDamagePrecommitCompletion::Direct {
            entity_id,
            game_mode,
            player_pose,
            attacker_session,
            dragon_part,
        } = completion
        {
            let mut inner = self.lock_session_entities("resume player attack server entity");
            let Some(ResolvedServerAttackTarget {
                snapshot: target,
                reach_position,
                reach_aabb,
                dragon_part: current_dragon_part,
            }) = resolve_server_attack_target_locked(&inner, entity_id)
            else {
                return (EntityDamagePrecommitResult::Direct, Vec::new());
            };
            if target != expected
                || target.item_stack.is_some()
                || current_dragon_part != dragon_part
            {
                return (EntityDamagePrecommitResult::Direct, Vec::new());
            }
            if attacker_costs
                .as_ref()
                .is_some_and(|(session, _)| Some(*session) != attacker_session)
            {
                return (EntityDamagePrecommitResult::Direct, Vec::new());
            }
            let attacker_persistence = attacker_session
                .and_then(|session| inner.player_persistence.get(&session).cloned());
            let (attacker_uuid, mut attacker_state) = if let Some(attacker_session) =
                attacker_session
            {
                let Some(session) = inner.sessions.get(&attacker_session) else {
                    return (EntityDamagePrecommitResult::Direct, Vec::new());
                };
                if session.pose != player_pose {
                    return (EntityDamagePrecommitResult::Direct, Vec::new());
                }
                let Some(persistence) = attacker_persistence.as_ref() else {
                    return (EntityDamagePrecommitResult::Direct, Vec::new());
                };
                let wait_started = Instant::now();
                let state =
                    lock_authoritative_mutex(persistence.as_ref(), "session_player_persistence");
                let state = crate::lock_metrics::timed_guard(
                    crate::lock_metrics::LockMetricKind::PlayerPersistence,
                    "resume player attack server entity",
                    wait_started,
                    state,
                );
                if state.game_mode != game_mode
                    || state.game_mode == GameMode::Spectator
                    || state.survival.is_dead()
                    || attacker_costs
                        .as_ref()
                        .is_some_and(|(_, costs)| !player_attack_cost_plan_matches(&state, costs))
                {
                    return (EntityDamagePrecommitResult::Direct, Vec::new());
                }
                (Some(session.uuid), Some(state))
            } else {
                (None, None)
            };
            let attack_range = attacker_state
                .as_deref()
                .and_then(|state| held_attack_range(&inner.player_combat, state));
            if game_mode == GameMode::Spectator
                || !within_entity_attack_reach(
                    player_pose,
                    reach_position,
                    reach_aabb,
                    game_mode,
                    attack_range,
                )
                || approval.consume().is_err()
            {
                return (EntityDamagePrecommitResult::Direct, Vec::new());
            }
            let rewards = entity_kill_rewards_locked(&inner, &target);
            let knockback_origin = (game_mode == GameMode::Survival).then_some(Vec3::new(
                player_pose.x,
                player_pose.y,
                player_pose.z,
            ));
            let gossip_event = (target.type_name == "minecraft:villager")
                .then_some(attacker_uuid)
                .flatten()
                .map(|player| {
                    mc_entity::villager_gossip_26_1_2::VillagerGossipEvent::HurtByPlayer { player }
                });
            let outcome = if let Some(part) = dragon_part {
                attack_dragon_part_locked(&mut inner, target, part, request.amount)
            } else {
                attack_server_entity_locked(
                    &mut inner,
                    target.id,
                    request.amount,
                    knockback_origin,
                    &rewards,
                    gossip_event,
                )
            };
            let Some(mut outcome) = outcome else {
                return (EntityDamagePrecommitResult::Direct, Vec::new());
            };
            let committed_attacker = attacker_costs.as_ref().zip(attacker_state.as_mut()).map(
                |((attacker_session, costs), attacker_state)| {
                    let mut effective = costs.clone();
                    effective.expected_survival = attacker_state.survival;
                    effective.updated_survival.health = attacker_state.survival.health;
                    let committed = apply_player_survival_plan_locked(
                        &mut inner,
                        *attacker_session,
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
            if let (Some(attacker_session), EntityAttackOutcome::Killed { entity, .. }) =
                (attacker_session, &outcome)
            {
                push_player_entity_killed_event_locked(
                    &inner,
                    attacker_session,
                    game_mode,
                    player_pose,
                    entity,
                );
            }
            let mut dispatches = std::mem::take(outcome.dispatches_mut());
            drop(attacker_state);
            drop(inner);
            self.append_spawned_xp_pickup_candidates(&mut dispatches);
            return (EntityDamagePrecommitResult::Direct, dispatches);
        }
        if let EntityDamagePrecommitCompletion::Effect {
            request: mut effect_request,
        } = completion
        {
            if !replace_entity_effect_damage_amount(&mut effect_request, request.amount) {
                return (
                    EntityDamagePrecommitResult::Effect(EntityEffectResult::Rejected(
                        EntityEffectRejection::InvalidAction,
                    )),
                    Vec::new(),
                );
            }
            let mut inner = self.lock_session_entities("resume entity effect damage precommit");
            if inner.entities.snapshot(expected.id).as_ref() != Some(&expected)
                || approval.consume().is_err()
            {
                return (
                    EntityDamagePrecommitResult::Effect(EntityEffectResult::Rejected(
                        EntityEffectRejection::Stale,
                    )),
                    Vec::new(),
                );
            }
            let (result, dispatches) =
                apply_server_entity_effect_request_locked(&mut inner, expected, effect_request);
            return (EntityDamagePrecommitResult::Effect(result), dispatches);
        }
        let mut inner = self.lock_session_entities("resume entity damage precommit");
        let attacker_persistence = attacker_costs
            .as_ref()
            .and_then(|(session, _)| inner.player_persistence.get(session).cloned());
        let mut attacker_state = if let Some((attacker_session, costs)) = attacker_costs.as_ref() {
            let Some(persistence) = attacker_persistence.as_ref() else {
                return (entity_damage_precommit_refusal(&completion), Vec::new());
            };
            let wait_started = Instant::now();
            let state = lock_authoritative_mutex(persistence, "session_player_persistence");
            let state = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::PlayerPersistence,
                "resume server-entity attack costs",
                wait_started,
                state,
            );
            if !player_attack_cost_plan_matches(&state, costs) || state.survival.is_dead() {
                return (entity_damage_precommit_refusal(&completion), Vec::new());
            }
            Some((*attacker_session, costs, state))
        } else {
            None
        };
        if inner.entities.snapshot(expected.id).as_ref() != Some(&expected)
            || approval.consume().is_err()
        {
            return (entity_damage_precommit_refusal(&completion), Vec::new());
        }
        let Some(damage) = inner.entities.damage_if_current(expected, request) else {
            return (entity_damage_precommit_refusal(&completion), Vec::new());
        };
        if let Some((attacker_session, costs, state)) = attacker_state.as_mut() {
            let mut effective = (*costs).clone();
            effective.expected_survival = state.survival;
            effective.updated_survival.health = state.survival.health;
            let _ =
                apply_player_survival_plan_locked(&mut inner, *attacker_session, state, &effective);
        }
        let mut dispatches = publish_accepted_entity_health_locked(&mut inner, &damage.snapshot);
        if damage.killed {
            let rewards = entity_kill_rewards_locked(&inner, &damage.snapshot);
            let (_, mut death_dispatches) =
                begin_server_entity_death_locked(&mut inner, &damage, &rewards);
            death_dispatches.splice(0..0, dispatches);
            dispatches = death_dispatches;
        } else {
            dispatches.extend(entity_hurt_dispatches_locked(&inner, damage.snapshot.id));
        }
        let result = match completion {
            EntityDamagePrecommitCompletion::Script => {
                EntityDamagePrecommitResult::Script(Some((damage.snapshot.health, damage.killed)))
            }
            EntityDamagePrecommitCompletion::Resident { prior_health } => {
                EntityDamagePrecommitResult::Resident(Some(super::resident_orders::ResidentHit {
                    damage: (prior_health - damage.snapshot.health).max(0.0),
                    killed: damage.killed,
                }))
            }
            EntityDamagePrecommitCompletion::Direct { .. }
            | EntityDamagePrecommitCompletion::Effect { .. }
            | EntityDamagePrecommitCompletion::Projectile(_)
            | EntityDamagePrecommitCompletion::Explosion(_)
            | EntityDamagePrecommitCompletion::Golem(_) => unreachable!(),
        };
        drop(attacker_state);
        drop(inner);
        self.append_spawned_xp_pickup_candidates(&mut dispatches);
        (result, dispatches)
    }

    pub(in crate::play) fn damage_script_entity(
        &self,
        _authority: &SimulationAuthority,
        entity_id: EntityId,
        amount: f32,
        plugin_id: &str,
        response: &mut Option<SimulationResponseSender>,
    ) -> Option<EntityAttackOutcome> {
        if self
            .precommit_boundary()
            .is_some_and(|boundary| boundary.has_precommit_hooks(HookKind::Damage))
        {
            let (expected, request) = {
                let inner = self.lock_session_entities("freeze script entity damage");
                let expected = inner.entities.snapshot(entity_id)?;
                if expected.item_stack.is_some()
                    || server_entity_snapshot_from(expected.clone())
                        .health
                        .is_none()
                {
                    return None;
                }
                (
                    expected,
                    EntityDamageRequest {
                        amount,
                        tick: inner.entity_lifecycle_tick,
                        death_remove_tick: inner
                            .entity_lifecycle_tick
                            .saturating_add(ENTITY_DEATH_TICKS),
                        villager_gossip_event: None,
                    },
                )
            };
            if defer_entity_damage_precommit(
                self,
                DeferredEntityDamage {
                    expected: expected.clone(),
                    request,
                    source: HookActor::Plugin(plugin_id.to_owned()),
                    kind: "script",
                    position: expected.position,
                    attacker_costs: None,
                    response: response.take(),
                    completion: EntityDamagePrecommitCompletion::Script,
                },
            ) {
                return None;
            }
            unreachable!("a registered damage hook must either defer or refuse script damage");
        }
        let mut outcome = {
            let mut inner = self.lock_session_entities("damage script entity");
            let expected = inner.entities.snapshot(entity_id)?;
            if expected.item_stack.is_some()
                || server_entity_snapshot_from(expected.clone())
                    .health
                    .is_none()
            {
                return None;
            }
            let rewards = entity_kill_rewards_locked(&inner, &expected);
            attack_server_entity_locked(&mut inner, entity_id, amount, None, &rewards, None)?
        };
        self.append_spawned_xp_pickup_candidates(outcome.dispatches_mut());
        Some(outcome)
    }

    pub(in crate::play) fn damage_resident_entity(
        &self,
        _authority: &SimulationAuthority,
        attack: super::resident_orders::ResidentAttack,
        plugin_id: &str,
        response: &mut Option<SimulationResponseSender>,
    ) -> Option<super::resident_orders::ResidentHit> {
        if self
            .precommit_boundary()
            .is_some_and(|boundary| boundary.has_precommit_hooks(HookKind::Damage))
        {
            let tick = self.simulation_tick();
            let request = EntityDamageRequest {
                amount: attack.amount,
                tick,
                death_remove_tick: tick.saturating_add(ENTITY_DEATH_TICKS),
                villager_gossip_event: None,
            };
            if defer_entity_damage_precommit(
                self,
                DeferredEntityDamage {
                    expected: attack.expected.clone(),
                    request,
                    source: HookActor::Plugin(plugin_id.to_owned()),
                    kind: "resident",
                    position: attack.expected.position,
                    attacker_costs: None,
                    response: response.take(),
                    completion: EntityDamagePrecommitCompletion::Resident {
                        prior_health: attack.expected.health.max(0.0),
                    },
                },
            ) {
                return None;
            }
            unreachable!("a registered damage hook must defer or refuse resident damage");
        }
        let mut inner = self.lock_session_entities("damage resident entity");
        if inner.entities.snapshot(attack.expected.id).as_ref() != Some(&attack.expected) {
            return None;
        }
        let request = EntityDamageRequest {
            amount: attack.amount,
            tick: inner.entity_lifecycle_tick,
            death_remove_tick: inner
                .entity_lifecycle_tick
                .saturating_add(ENTITY_DEATH_TICKS),
            villager_gossip_event: None,
        };
        let damage = inner
            .entities
            .damage_if_current(attack.expected.clone(), request)?;
        let mut dispatches = publish_accepted_entity_health_locked(&mut inner, &damage.snapshot);
        if damage.killed {
            let rewards = entity_kill_rewards_locked(&inner, &damage.snapshot);
            let (_, mut death_dispatches) =
                begin_server_entity_death_locked(&mut inner, &damage, &rewards);
            death_dispatches.splice(0..0, dispatches);
            dispatches = death_dispatches;
        } else {
            dispatches.extend(entity_hurt_dispatches_locked(&inner, damage.snapshot.id));
        }
        drop(inner);
        self.append_spawned_xp_pickup_candidates(&mut dispatches);
        let hit = super::resident_orders::ResidentHit {
            damage: (attack.expected.health.max(0.0) - damage.snapshot.health).max(0.0),
            killed: damage.killed,
        };
        super::dispatch_visibility_commands(dispatches);
        Some(hit)
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
            entity: Box::new(entity),
            dispatches,
            attacker_costs: None,
        });
    }
    let mut dispatches = health_dispatches;
    dispatches.extend(entity_hurt_dispatches_locked(inner, entity_id));
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

/// Vanilla `LivingEntity#fall` / `calculateFallDamage`: one damage point per
/// block beyond the three-block safe height, rounded up to whole points.
#[must_use]
pub(super) fn entity_fall_damage_amount(fall_distance: f64) -> Option<f32> {
    if !fall_distance.is_finite() || fall_distance <= super::entity_owner::ENTITY_FALL_SAFE_HEIGHT {
        return None;
    }
    Some((fall_distance - super::entity_owner::ENTITY_FALL_SAFE_HEIGHT).ceil() as f32)
}

/// Apply vanilla fall damage for landings observed on the committed region
/// physics stream and fire the matching `EntityHurt` (or death) feedback.
pub(super) fn resolve_entity_fall_damage(
    sessions: &SessionRegistry,
    landings: &[super::entity_owner::EntityFallLanding],
) {
    if landings.is_empty() {
        return;
    }
    let mut dispatches = {
        let mut inner = sessions.lock_session_entities("entity fall damage");
        let mut dispatches = Vec::new();
        for landing in landings {
            let Some(amount) = entity_fall_damage_amount(landing.fall_distance) else {
                continue;
            };
            let Some(expected) = inner.entities.snapshot(landing.id) else {
                continue;
            };
            // Items, XP orbs, arrows and healthless specials never take fall
            // damage; the tracker already skips the streaming exceptions.
            if expected.item_stack.is_some()
                || expected.lifecycle != EntityLifecycle::Alive
                || server_entity_snapshot_from(expected.clone())
                    .health
                    .is_none()
            {
                continue;
            }
            let rewards = entity_kill_rewards_locked(&inner, &expected);
            if let Some(mut outcome) =
                attack_server_entity_locked(&mut inner, landing.id, amount, None, &rewards, None)
            {
                dispatches.append(outcome.dispatches_mut());
            }
        }
        dispatches
    };
    sessions.append_spawned_xp_pickup_candidates(&mut dispatches);
    super::dispatch_visibility_commands(dispatches);
}
#[cfg(test)]
mod fall_damage_tests {
    use super::*;

    #[test]
    fn fall_damage_amounts_follow_vanilla_safe_height() {
        assert_eq!(entity_fall_damage_amount(3.0), None);
        assert_eq!(entity_fall_damage_amount(2.5), None);
        assert_eq!(entity_fall_damage_amount(f64::NAN), None);
        assert_eq!(entity_fall_damage_amount(3.5), Some(1.0));
        assert_eq!(entity_fall_damage_amount(4.0), Some(1.0));
        assert_eq!(entity_fall_damage_amount(10.0), Some(7.0));
    }
}
