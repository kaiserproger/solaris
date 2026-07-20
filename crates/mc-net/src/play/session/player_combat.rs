use super::interaction_geometry::within_entity_reach;
use super::outbound::{
    OutboundCommand, PlayerCarriedItemDelta, PlayerDamagePublication, PlayerHurtEvent,
    PlayerInventorySlotDelta, PlayerXpDelta, SessionRecipient, VisibilityDispatch,
};
use super::player_state::{
    apply_player_survival_plan_locked, player_attack_cost_plan_matches,
    player_survival_plan_matches,
};
use super::sleep::{
    SleepWakeReason, SleepingPlayer, defer_staged_sleep_dispatches_locked, stage_sleep_wake_locked,
};
use super::{
    CommittedPlayerAttackCosts, EntityAttackOutcome, PlayerAttackResult, ServerEntityPlayerAttack,
    SessionEntityGuards, SessionId, SessionRegistry, player_aabb, session_recipients,
    visible_observers_locked,
};
use crate::play::combat::{
    ActiveShield, PlayerDamageKind, PlayerDamageRequest, PlayerHurtResolution,
    damage_active_shield_slot, melee_knockback, shield_block_knockback, shield_blocks_damage_since,
    shield_use_matches_slot,
};
use crate::play::inventory::{
    damage_inventory_armor, inventory_damage_after_armor, inventory_damage_after_protection,
};
use crate::play::simulation::{PlayerSurvivalPlan, SimulationAuthority};
use crate::play::{GameMode, PlayerPose};
use mc_entity::{EntityId, Vec3};
use std::sync::Arc;
use std::time::Instant;
use tracing::warn;

pub(in crate::play) struct PlayerEntityAttack<'a> {
    pub(in crate::play) attacker_session: SessionId,
    pub(in crate::play) entity_id: EntityId,
    pub(in crate::play) amount: f32,
    pub(in crate::play) attacker_costs: Option<&'a PlayerSurvivalPlan>,
    pub(in crate::play) authority_tick: u64,
}

struct PlayerAttackCommit<'a> {
    attacker_session: SessionId,
    target_session: SessionId,
    expected_attacker_mode: GameMode,
    attacker_costs: Option<&'a PlayerSurvivalPlan>,
    expected_shield: Option<ActiveShield>,
    shield_after_block: Option<ShieldAfterBlock>,
    next_resistance: Option<crate::play::combat::PlayerHurtResistance>,
    target_plan: &'a PlayerSurvivalPlan,
}

enum ShieldAfterBlock {
    Refresh(ActiveShield),
    Remove,
}

impl SessionRegistry {
    pub(in crate::play) fn player_attack_entity(
        &self,
        authority: &SimulationAuthority,
        attack: PlayerEntityAttack<'_>,
    ) -> PlayerAttackResult {
        let PlayerEntityAttack {
            attacker_session,
            entity_id,
            amount,
            attacker_costs,
            authority_tick,
        } = attack;
        if !amount.is_finite() || amount <= 0.0 {
            return PlayerAttackResult::ValidationRejected;
        }

        let inner = self.lock_inner("player attack target");
        let Some(attacker_pose) = inner
            .sessions
            .get(&attacker_session)
            .map(|session| session.pose)
        else {
            return PlayerAttackResult::ValidationRejected;
        };
        let Some(attacker_state) = inner.player_persistence.get(&attacker_session).cloned() else {
            return PlayerAttackResult::ValidationRejected;
        };
        let target_session = inner.sessions.iter().find_map(|(&session_id, session)| {
            (session.entity_id == entity_id.0).then_some(session_id)
        });
        let Some(target_session) = target_session else {
            drop(inner);
            let Some(authoritative_mode) = authoritative_attacker_mode(
                attacker_session,
                &attacker_state,
                "validate server-entity attacker state",
            ) else {
                return PlayerAttackResult::ValidationRejected;
            };
            return self.player_attack_server_entity(
                authority,
                ServerEntityPlayerAttack {
                    entity_id,
                    amount,
                    game_mode: authoritative_mode,
                    player_pose: attacker_pose,
                    attacker: attacker_costs.map(|costs| (attacker_session, costs)),
                },
            );
        };
        if target_session == attacker_session {
            return PlayerAttackResult::ValidationRejected;
        }

        let Some(target) = inner.sessions.get(&target_session) else {
            return PlayerAttackResult::ValidationRejected;
        };
        let target_pose = target.pose;
        let target_position = Vec3::new(target_pose.x, target_pose.y, target_pose.z);
        let Some(target_state) = inner.player_persistence.get(&target_session).cloned() else {
            return PlayerAttackResult::ValidationRejected;
        };
        let target_recipient = SessionRecipient::unordered(
            target_session,
            target.tx.clone(),
            Arc::clone(&target.pressure),
        );
        let hurt_event = PlayerHurtEvent {
            entity_id: target.entity_id,
        };
        let hurt_entity_id = hurt_event.entity_id;
        let hurt_observers =
            session_recipients(&inner, visible_observers_locked(&inner, target_session));
        let active_shield = inner.active_shields.get(&target_session).cloned();
        let current_resistance = inner
            .player_hurt_resistance
            .get(&target_session)
            .copied()
            .unwrap_or_default();
        let combat_resources = inner.player_combat.clone();
        drop(inner);

        let Some(authoritative_mode) = authoritative_attacker_mode(
            attacker_session,
            &attacker_state,
            "validate PvP attacker state",
        ) else {
            return PlayerAttackResult::ValidationRejected;
        };
        if !within_entity_reach(
            attacker_pose,
            target_position,
            player_aabb(),
            authoritative_mode,
        ) {
            return PlayerAttackResult::ValidationRejected;
        }

        let wait_started = Instant::now();
        let target_state = target_state.lock().unwrap_or_else(|poisoned| {
            warn!(
                session_id = target_session,
                "player persistence mutex was poisoned during PvP target validation; recovering state"
            );
            poisoned.into_inner()
        });
        let target_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "validate PvP target state",
            wait_started,
            target_state,
        );
        if matches!(
            target_state.game_mode,
            GameMode::Creative | GameMode::Spectator
        ) || target_state.survival.is_dead()
        {
            return PlayerAttackResult::ValidationRejected;
        }
        let expected = target_state.clone();
        drop(target_state);

        let damage = PlayerDamageRequest {
            kind: PlayerDamageKind::PlayerAttack,
            amount,
            source_origin: Some(Vec3::new(attacker_pose.x, attacker_pose.y, attacker_pose.z)),
        };
        let source_origin = damage.source_origin;
        let items = combat_resources.items.as_ref();
        let shield_blocks = active_shield.as_ref().is_some_and(|shield| {
            active_shield_blocks_player_damage(
                target_pose,
                source_origin,
                self.world_time(),
                shield,
                items,
                &expected.inventory,
            )
        });

        let mut updated_inventory = expected.inventory.clone();
        let mut updated_survival = expected.survival;
        let damage_applied;
        let fresh_hurt;
        let next_resistance;
        let shield_after_block;
        if shield_blocks {
            let shield = active_shield.as_ref().expect("shield was checked above");
            shield_after_block = match damage_active_shield_slot(
                items,
                &combat_resources.item_facts,
                &mut updated_inventory.slots,
                shield.slot,
                &shield.expected_stack,
                damage.amount,
            ) {
                Some((_, _, true)) => Some(ShieldAfterBlock::Remove),
                Some((_, updated_stack, false)) => {
                    let mut refreshed = shield.clone();
                    refreshed.expected_stack = updated_stack;
                    Some(ShieldAfterBlock::Refresh(refreshed))
                }
                None => Some(ShieldAfterBlock::Refresh(shield.clone())),
            };
            damage_applied = false;
            fresh_hurt = false;
            next_resistance = None;
        } else {
            shield_after_block = None;
            let (resolution, resolved_resistance) =
                current_resistance.preview(authority_tick, damage.amount);
            let PlayerHurtResolution::Apply {
                amount: resolved_damage,
                fresh_hurt: resolved_fresh_hurt,
            } = resolution
            else {
                return committed_player_attack_without_damage(target_session);
            };
            let armor_damage =
                inventory_damage_after_armor(items, &expected.inventory, resolved_damage);
            let applied_damage =
                inventory_damage_after_protection(items, &expected.inventory, armor_damage);
            if applied_damage <= 0.0 {
                return committed_player_attack_without_damage(target_session);
            }
            updated_survival.apply_damage(applied_damage);
            damage_inventory_armor(items, &mut updated_inventory, resolved_damage);
            damage_applied = true;
            fresh_hurt = resolved_fresh_hurt;
            next_resistance = Some(resolved_resistance);
        }

        let target_plan = PlayerSurvivalPlan {
            expected_survival: expected.survival,
            updated_survival,
            expected_inventory: expected.inventory,
            updated_inventory,
            expected_carried_item: expected.carried_item,
            expected_xp: expected.xp.clone(),
            updated_xp: expected.xp,
            active_shield: None,
            enchanting_table_input: None,
            item_entity_type_id: combat_resources.item_entity_type_id,
            xp_orb_entity_type_id: combat_resources.xp_orb_entity_type_id,
            position: target_position,
        };
        let Some((mut committed, committed_attacker_costs, staged_damage_wake)) = self
            .commit_player_attack(
                authority,
                PlayerAttackCommit {
                    attacker_session,
                    target_session,
                    expected_attacker_mode: authoritative_mode,
                    attacker_costs: if damage_applied { attacker_costs } else { None },
                    expected_shield: active_shield,
                    shield_after_block,
                    next_resistance,
                    target_plan: &target_plan,
                },
            )
        else {
            return committed_player_attack_without_damage(target_session);
        };

        let publication = PlayerDamagePublication {
            expected_health: target_plan.expected_survival.health,
            health: committed.survival.health,
            inventory: target_plan
                .expected_inventory
                .slots
                .iter()
                .zip(&committed.inventory.slots)
                .enumerate()
                .filter(|(_, (expected, updated))| expected != updated)
                .map(|(slot, (expected, updated))| PlayerInventorySlotDelta {
                    slot,
                    expected: expected.clone(),
                    updated: updated.clone(),
                })
                .collect(),
            carried_item: (target_plan.expected_carried_item != committed.carried_item).then(
                || PlayerCarriedItemDelta {
                    expected: target_plan.expected_carried_item.clone(),
                    updated: committed.carried_item,
                },
            ),
            xp: (target_plan.expected_xp != committed.xp).then(|| PlayerXpDelta {
                expected: target_plan.expected_xp.clone(),
                updated: committed.xp,
            }),
            died: committed.died,
            fresh_hurt: damage_applied && fresh_hurt,
            shield_blocked: shield_blocks,
            knockback: source_origin.and_then(|source| {
                if shield_blocks {
                    shield_block_knockback(
                        target_pose.x,
                        target_pose.z,
                        target_pose.flags.on_ground,
                        source,
                    )
                } else if damage_applied && fresh_hurt {
                    melee_knockback(
                        target_pose.x,
                        target_pose.z,
                        target_pose.flags.on_ground,
                        source,
                    )
                } else {
                    None
                }
            }),
        };
        let mut dispatches = std::mem::take(&mut committed.dispatches);
        dispatches.push(VisibilityDispatch {
            recipient: target_recipient,
            command: OutboundCommand::PlayerDamageCommitted {
                publication: Box::new(publication),
                hurt_event,
            },
        });
        if damage_applied && fresh_hurt {
            dispatches.extend(
                hurt_observers
                    .into_iter()
                    .map(|recipient| VisibilityDispatch {
                        recipient,
                        command: OutboundCommand::EntityEvent {
                            entity_id: hurt_entity_id,
                            event_id: 2,
                        },
                    }),
            );
        }
        if let Some(sleeper) = staged_damage_wake {
            self.defer_staged_sleep_dispatches(target_session, &mut dispatches);
            dispatches = self.completed_sleep_dispatches(vec![sleeper], None);
        }
        PlayerAttackResult::Damaged(Box::new(EntityAttackOutcome::PlayerDamaged {
            target_session,
            dispatches,
            damage_applied,
            attacker_costs: committed_attacker_costs,
        }))
    }

    fn commit_player_attack(
        &self,
        _authority: &SimulationAuthority,
        commit: PlayerAttackCommit<'_>,
    ) -> Option<(
        crate::play::simulation::CommittedPlayerSurvival,
        Option<CommittedPlayerAttackCosts>,
        Option<SleepingPlayer>,
    )> {
        let PlayerAttackCommit {
            attacker_session,
            target_session,
            expected_attacker_mode,
            attacker_costs,
            expected_shield,
            shield_after_block,
            next_resistance,
            target_plan,
        } = commit;
        let mut inner = self.lock_session_entities("commit player attack");
        if !inner.sessions.contains_key(&attacker_session)
            || !inner.sessions.contains_key(&target_session)
            || inner.active_shields.get(&target_session) != expected_shield.as_ref()
        {
            return None;
        }
        let attacker = inner.player_persistence.get(&attacker_session)?.clone();
        let target = inner.player_persistence.get(&target_session)?.clone();
        let (first_id, first, second_id, second) = if attacker_session < target_session {
            (attacker_session, &attacker, target_session, &target)
        } else {
            (target_session, &target, attacker_session, &attacker)
        };
        let first_wait = Instant::now();
        let first_guard = first
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let first_guard = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit player attack first participant",
            first_wait,
            first_guard,
        );
        let second_wait = Instant::now();
        let second_guard = second
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let second_guard = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit player attack second participant",
            second_wait,
            second_guard,
        );
        let (mut attacker_state, mut target_state) = if first_id == attacker_session {
            (first_guard, second_guard)
        } else {
            debug_assert_eq!(second_id, attacker_session);
            (second_guard, first_guard)
        };
        if attacker_state.game_mode != expected_attacker_mode
            || attacker_state.game_mode == GameMode::Spectator
            || attacker_state.survival.is_dead()
        {
            return None;
        }
        if !player_survival_plan_matches(&target_state, target_plan) {
            return None;
        }
        if let Some(costs) = attacker_costs
            && (!player_attack_cost_plan_matches(&attacker_state, costs))
        {
            return None;
        }

        let staged_damage_wake = (target_plan.updated_survival.health
            < target_plan.expected_survival.health)
            .then(|| {
                self.stage_sleep_wake_locked(&mut inner, target_session, SleepWakeReason::Damage)
            })
            .flatten();

        let committed_attacker = attacker_costs.map(|costs| {
            let mut effective = costs.clone();
            effective.expected_survival = attacker_state.survival;
            effective.updated_survival.health = attacker_state.survival.health;
            let committed =
                apply_player_survival_plan_locked(&mut inner, &mut attacker_state, &effective);
            CommittedPlayerAttackCosts {
                survival: committed.survival,
                inventory: committed.inventory,
            }
        });
        let mut committed_target =
            apply_player_survival_plan_locked(&mut inner, &mut target_state, target_plan);
        if let Some(shield_after_block) = shield_after_block {
            match shield_after_block {
                ShieldAfterBlock::Refresh(shield) => {
                    inner.active_shields.insert(target_session, shield);
                }
                ShieldAfterBlock::Remove => {
                    inner.active_shields.remove(&target_session);
                }
            }
        }
        if let Some(next_resistance) = next_resistance {
            inner
                .player_hurt_resistance
                .insert(target_session, next_resistance);
        }
        drop(target_state);
        drop(attacker_state);
        drop(inner);
        self.append_spawned_xp_pickup_candidates(&mut committed_target.dispatches);
        Some((committed_target, committed_attacker, staged_damage_wake))
    }
}

fn authoritative_attacker_mode(
    attacker_session: SessionId,
    attacker_state: &std::sync::Mutex<crate::play::persistence::PlayerPersistedState>,
    operation: &'static str,
) -> Option<GameMode> {
    let wait_started = Instant::now();
    let state = attacker_state.lock().unwrap_or_else(|poisoned| {
        warn!(
            session_id = attacker_session,
            "player persistence mutex was poisoned during attacker validation; recovering state"
        );
        poisoned.into_inner()
    });
    let state = crate::lock_metrics::timed_guard(
        crate::lock_metrics::LockMetricKind::PlayerPersistence,
        operation,
        wait_started,
        state,
    );
    (!state.survival.is_dead() && state.game_mode != GameMode::Spectator).then_some(state.game_mode)
}

fn committed_player_attack_without_damage(target_session: SessionId) -> PlayerAttackResult {
    PlayerAttackResult::Damaged(Box::new(EntityAttackOutcome::PlayerDamaged {
        target_session,
        dispatches: Vec::new(),
        damage_applied: false,
        attacker_costs: None,
    }))
}

fn active_shield_blocks_player_damage(
    pose: PlayerPose,
    source_origin: Option<Vec3>,
    current_tick: u64,
    shield: &ActiveShield,
    items: &mc_data::items::ItemRegistry,
    inventory: &crate::play::inventory::PlayerInventory,
) -> bool {
    if !shield_use_matches_slot(
        shield.slot,
        shield.slot,
        &shield.expected_stack,
        &inventory.slots,
        items,
    ) {
        return false;
    }
    shield_blocks_damage_since(
        Vec3::new(pose.x, pose.y, pose.z),
        pose.yaw,
        source_origin,
        current_tick,
        shield.started_tick,
    )
}

pub(super) enum ProjectilePlayerDamagePreview {
    Accepted(PreparedProjectilePlayerDamage),
    Rejected(Option<PreparedProjectilePlayerDamage>),
}

pub(super) struct PreparedProjectilePlayerDamage {
    target_session: SessionId,
    expected_shield: Option<ActiveShield>,
    shield_after_block: Option<ShieldAfterBlock>,
    next_resistance: Option<crate::play::combat::PlayerHurtResistance>,
    target_plan: PlayerSurvivalPlan,
    damage_applied: bool,
    fresh_hurt: bool,
    shield_blocked: bool,
    source_origin: Option<Vec3>,
}

impl PreparedProjectilePlayerDamage {
    pub(super) fn kills_player(&self) -> bool {
        !self.target_plan.expected_survival.is_dead() && self.target_plan.updated_survival.is_dead()
    }
}

pub(super) fn prepare_projectile_player_damage_locked(
    inner: &SessionEntityGuards<'_>,
    target_session: SessionId,
    current_tick: u64,
    damage: PlayerDamageRequest,
) -> ProjectilePlayerDamagePreview {
    let Some(target) = inner.sessions.get(&target_session) else {
        return ProjectilePlayerDamagePreview::Rejected(None);
    };
    let target_pose = target.pose;
    let target_position = Vec3::new(target_pose.x, target_pose.y, target_pose.z);
    let Some(target_state) = inner.player_persistence.get(&target_session).cloned() else {
        return ProjectilePlayerDamagePreview::Rejected(None);
    };
    let wait_started = Instant::now();
    let target_state = target_state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let target_state = crate::lock_metrics::timed_guard(
        crate::lock_metrics::LockMetricKind::PlayerPersistence,
        "prepare projectile player damage",
        wait_started,
        target_state,
    );
    if matches!(
        target_state.game_mode,
        GameMode::Creative | GameMode::Spectator
    ) || target_state.survival.is_dead()
    {
        return ProjectilePlayerDamagePreview::Rejected(None);
    }

    let active_shield = inner.active_shields.get(&target_session).cloned();
    let combat_resources = &inner.player_combat;
    let items = combat_resources.items.as_ref();
    let shield_blocks = damage.kind.can_be_blocked_by_shield()
        && active_shield.as_ref().is_some_and(|shield| {
            active_shield_blocks_player_damage(
                target_pose,
                damage.source_origin,
                current_tick,
                shield,
                items,
                &target_state.inventory,
            )
        });
    let mut updated_inventory = target_state.inventory.clone();
    let mut updated_survival = target_state.survival;
    let (damage_applied, fresh_hurt, next_resistance, shield_after_block) = if shield_blocks {
        let shield = active_shield.as_ref().expect("shield was checked above");
        let shield_after_block = match damage_active_shield_slot(
            items,
            &combat_resources.item_facts,
            &mut updated_inventory.slots,
            shield.slot,
            &shield.expected_stack,
            damage.amount,
        ) {
            Some((_, _, true)) => Some(ShieldAfterBlock::Remove),
            Some((_, updated_stack, false)) => {
                let mut refreshed = shield.clone();
                refreshed.expected_stack = updated_stack;
                Some(ShieldAfterBlock::Refresh(refreshed))
            }
            None => Some(ShieldAfterBlock::Refresh(shield.clone())),
        };
        (false, false, None, shield_after_block)
    } else {
        let current_resistance = inner
            .player_hurt_resistance
            .get(&target_session)
            .copied()
            .unwrap_or_default();
        let (resolution, next_resistance) = current_resistance.preview(current_tick, damage.amount);
        let PlayerHurtResolution::Apply {
            amount: resolved_damage,
            fresh_hurt,
        } = resolution
        else {
            return ProjectilePlayerDamagePreview::Rejected(None);
        };
        let armor_damage =
            inventory_damage_after_armor(items, &target_state.inventory, resolved_damage);
        let applied_damage =
            inventory_damage_after_protection(items, &target_state.inventory, armor_damage);
        if applied_damage <= 0.0 {
            return ProjectilePlayerDamagePreview::Rejected(None);
        }
        updated_survival.apply_damage(applied_damage);
        if damage.kind.damages_armor() {
            damage_inventory_armor(items, &mut updated_inventory, resolved_damage);
        }
        (true, fresh_hurt, Some(next_resistance), None)
    };

    let target_plan = PlayerSurvivalPlan {
        expected_survival: target_state.survival,
        updated_survival,
        expected_inventory: target_state.inventory.clone(),
        updated_inventory,
        expected_carried_item: target_state.carried_item.clone(),
        expected_xp: target_state.xp.clone(),
        updated_xp: target_state.xp.clone(),
        active_shield: None,
        enchanting_table_input: None,
        item_entity_type_id: combat_resources.item_entity_type_id,
        xp_orb_entity_type_id: combat_resources.xp_orb_entity_type_id,
        position: target_position,
    };
    let prepared = PreparedProjectilePlayerDamage {
        target_session,
        expected_shield: active_shield,
        shield_after_block,
        next_resistance,
        target_plan,
        damage_applied,
        fresh_hurt,
        shield_blocked: shield_blocks,
        source_origin: damage.source_origin,
    };
    if damage_applied {
        ProjectilePlayerDamagePreview::Accepted(prepared)
    } else {
        ProjectilePlayerDamagePreview::Rejected(Some(prepared))
    }
}

pub(super) fn commit_projectile_player_damage_locked(
    inner: &mut SessionEntityGuards<'_>,
    prepared: PreparedProjectilePlayerDamage,
    commit_entities: impl FnOnce(&mut SessionEntityGuards<'_>) -> bool,
    dispatches: &mut Vec<VisibilityDispatch>,
) -> bool {
    let PreparedProjectilePlayerDamage {
        target_session,
        expected_shield,
        shield_after_block,
        next_resistance,
        target_plan,
        damage_applied,
        fresh_hurt,
        shield_blocked,
        source_origin,
    } = prepared;
    let Some(target) = inner.sessions.get(&target_session) else {
        return false;
    };
    let target_pose = target.pose;
    let target_entity_id = target.entity_id;
    if inner.active_shields.get(&target_session) != expected_shield.as_ref() {
        return false;
    }
    let Some(target_state) = inner.player_persistence.get(&target_session).cloned() else {
        return false;
    };
    let wait_started = Instant::now();
    let target_state = target_state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut target_state = crate::lock_metrics::timed_guard(
        crate::lock_metrics::LockMetricKind::PlayerPersistence,
        "commit projectile player damage",
        wait_started,
        target_state,
    );
    if !player_survival_plan_matches(&target_state, &target_plan) {
        return false;
    }
    if !commit_entities(inner) {
        return false;
    }
    let staged_damage_wake = (target_plan.updated_survival.health
        < target_plan.expected_survival.health)
        .then(|| stage_sleep_wake_locked(inner, target_session, SleepWakeReason::Damage))
        .flatten();
    let mut committed = apply_player_survival_plan_locked(inner, &mut target_state, &target_plan);
    if let Some(shield_after_block) = shield_after_block {
        match shield_after_block {
            ShieldAfterBlock::Refresh(shield) => {
                inner.active_shields.insert(target_session, shield);
            }
            ShieldAfterBlock::Remove => {
                inner.active_shields.remove(&target_session);
            }
        }
    }
    if let Some(next_resistance) = next_resistance {
        inner
            .player_hurt_resistance
            .insert(target_session, next_resistance);
    }
    let publication = PlayerDamagePublication {
        expected_health: target_plan.expected_survival.health,
        health: committed.survival.health,
        inventory: target_plan
            .expected_inventory
            .slots
            .iter()
            .zip(&committed.inventory.slots)
            .enumerate()
            .filter(|(_, (expected, updated))| expected != updated)
            .map(|(slot, (expected, updated))| PlayerInventorySlotDelta {
                slot,
                expected: expected.clone(),
                updated: updated.clone(),
            })
            .collect(),
        carried_item: (target_plan.expected_carried_item != committed.carried_item).then(|| {
            PlayerCarriedItemDelta {
                expected: target_plan.expected_carried_item.clone(),
                updated: committed.carried_item,
            }
        }),
        xp: (target_plan.expected_xp != committed.xp).then(|| PlayerXpDelta {
            expected: target_plan.expected_xp.clone(),
            updated: committed.xp,
        }),
        died: committed.died,
        fresh_hurt: damage_applied && fresh_hurt,
        shield_blocked,
        knockback: source_origin.and_then(|source| {
            if shield_blocked {
                shield_block_knockback(
                    target_pose.x,
                    target_pose.z,
                    target_pose.flags.on_ground,
                    source,
                )
            } else if damage_applied && fresh_hurt {
                melee_knockback(
                    target_pose.x,
                    target_pose.z,
                    target_pose.flags.on_ground,
                    source,
                )
            } else {
                None
            }
        }),
    };
    let mut damage_dispatches = std::mem::take(&mut committed.dispatches);
    let target = inner
        .sessions
        .get(&target_session)
        .expect("validated projectile target remains under the session guard");
    let target_recipient = SessionRecipient::unordered(
        target_session,
        target.tx.clone(),
        Arc::clone(&target.pressure),
    );
    let hurt_event = PlayerHurtEvent {
        entity_id: target_entity_id,
    };
    let hurt_observers = session_recipients(inner, visible_observers_locked(inner, target_session));
    damage_dispatches.push(VisibilityDispatch {
        recipient: target_recipient.clone(),
        command: OutboundCommand::PlayerDamageCommitted {
            publication: Box::new(publication),
            hurt_event,
        },
    });
    if damage_applied && fresh_hurt {
        damage_dispatches.extend(
            hurt_observers
                .into_iter()
                .map(|recipient| VisibilityDispatch {
                    recipient,
                    command: OutboundCommand::EntityEvent {
                        entity_id: target_entity_id,
                        event_id: 2,
                    },
                }),
        );
    }
    if let Some(sleeper) = staged_damage_wake {
        defer_staged_sleep_dispatches_locked(inner, target_session, &mut damage_dispatches);
        damage_dispatches.push(VisibilityDispatch {
            recipient: target_recipient,
            command: OutboundCommand::WakeFromBed { bed: sleeper.bed },
        });
    }
    dispatches.append(&mut damage_dispatches);
    true
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    use mc_data::Identifier;
    use mc_data::item_components::{ItemFacts, ItemFactsTable};
    use mc_data::items::{ItemRegistry, ItemReport};
    use mc_protocol::packets::play::{InteractionHand, ItemStack};
    use tokio::sync::mpsc;

    use super::{
        EntityAttackOutcome, EntityId, OutboundCommand, PlayerAttackCommit, PlayerAttackResult,
        PlayerEntityAttack, PreparedProjectilePlayerDamage, ShieldAfterBlock, VisibilityDispatch,
        commit_projectile_player_damage_locked,
    };
    use crate::login::LoggedInProfile;
    use crate::play::combat::{
        ActiveShield, ShieldUseState, damage_active_shield_slot, damage_active_shield_slots,
        shield_use_matches, shield_use_matches_slot,
    };
    use crate::play::persistence::PlayerPersistedState;
    use crate::play::session::SessionRegistry;
    use crate::play::simulation::{PlayerSurvivalPlan, SimulationAuthority};
    use crate::play::{GameMode, PlayerPose};

    fn shield_items() -> (ItemRegistry, ItemFactsTable, u32, u32) {
        let shield = Identifier::parse("minecraft:shield").unwrap();
        let stone = Identifier::parse("minecraft:stone").unwrap();
        let items = ItemRegistry::from_report(&[
            ItemReport {
                id: shield.clone(),
                protocol_id: 1,
            },
            ItemReport {
                id: stone,
                protocol_id: 2,
            },
        ]);
        let facts = ItemFactsTable::from_entries([(
            shield,
            ItemFacts {
                max_damage: Some(10),
                ..ItemFacts::default()
            },
        )]);
        (items, facts, 1, 2)
    }

    fn register_player(
        registry: &SessionRegistry,
        name: &str,
        pose: PlayerPose,
        state: PlayerPersistedState,
    ) -> u64 {
        let (tx, _rx) = mpsc::channel(1);
        let (session, _) = registry.register(
            &LoggedInProfile {
                uuid: crate::login::offline_uuid(name),
                name: name.to_owned(),
            },
            (0, 0),
            0,
            HashSet::new(),
            tx,
            pose,
        );
        registry.register_player_persistence(session, Arc::new(Mutex::new(state)));
        session
    }

    fn survival_plan(
        state: &PlayerPersistedState,
        updated_inventory: crate::play::inventory::PlayerInventory,
        pose: PlayerPose,
    ) -> PlayerSurvivalPlan {
        PlayerSurvivalPlan {
            expected_survival: state.survival,
            updated_survival: state.survival,
            expected_inventory: state.inventory.clone(),
            updated_inventory,
            expected_carried_item: state.carried_item.clone(),
            expected_xp: state.xp.clone(),
            updated_xp: state.xp.clone(),
            active_shield: None,
            enchanting_table_input: None,
            item_entity_type_id: None,
            xp_orb_entity_type_id: None,
            position: mc_entity::Vec3::new(pose.x, pose.y, pose.z),
        }
    }

    #[test]
    fn local_and_pvp_shields_reject_replaced_stack_identity() {
        let (items, _, shield, stone) = shield_items();
        let slot = 45;
        let mut slots = vec![ItemStack::EMPTY; 46];
        let shield_use = ShieldUseState {
            hand: InteractionHand::OffHand,
            started_tick: 1,
            slot,
            stack: ItemStack::new(shield, 1),
        };

        slots[slot] = shield_use.stack.clone();
        assert_eq!(
            shield_use_matches(&shield_use, slot, &slots, &items),
            shield_use_matches_slot(slot, slot, &shield_use.stack, &slots, &items),
        );
        assert!(shield_use_matches_slot(
            slot,
            slot,
            &shield_use.stack,
            &slots,
            &items,
        ));

        slots[slot] = ItemStack::new(shield, 1).with_damage(3);
        assert_eq!(
            shield_use_matches(&shield_use, slot, &slots, &items),
            shield_use_matches_slot(slot, slot, &shield_use.stack, &slots, &items),
        );
        assert!(!shield_use_matches_slot(
            slot,
            slot,
            &shield_use.stack,
            &slots,
            &items,
        ));

        slots[slot] = ItemStack::new(stone, 1);
        assert_eq!(
            shield_use_matches(&shield_use, slot, &slots, &items),
            shield_use_matches_slot(slot, slot, &shield_use.stack, &slots, &items),
        );
        assert!(!shield_use_matches_slot(
            slot,
            slot,
            &shield_use.stack,
            &slots,
            &items,
        ));
    }

    #[test]
    fn local_and_pvp_shields_share_durability_policy() {
        let (items, item_facts, shield, _) = shield_items();
        let slot = 45;
        let mut local_slots = vec![ItemStack::EMPTY; 46];
        local_slots[slot] = ItemStack::new(shield, 1);
        let mut local_use = ShieldUseState {
            hand: InteractionHand::OffHand,
            started_tick: 1,
            slot,
            stack: local_slots[slot].clone(),
        };
        let mut pvp_slots = local_slots.clone();
        let expected_stack = local_use.stack.clone();

        let local =
            damage_active_shield_slots(&items, &item_facts, &mut local_slots, &mut local_use, 4.2);
        let pvp = damage_active_shield_slot(
            &items,
            &item_facts,
            &mut pvp_slots,
            slot,
            &expected_stack,
            4.2,
        );

        assert_eq!(local, pvp);
        assert_eq!(local_slots, pvp_slots);
        assert!(shield_use_matches(&local_use, slot, &local_slots, &items));
        let pvp_stack = &pvp.as_ref().expect("PvP durability result").1;
        assert!(shield_use_matches_slot(
            slot, slot, pvp_stack, &pvp_slots, &items,
        ));
    }

    #[test]
    fn pvp_commit_refreshes_then_removes_authoritative_shield_identity() {
        let registry = SessionRegistry::new();
        let pose = PlayerPose::new(0.5, 64.0, 0.5);
        let attacker = register_player(
            &registry,
            "ShieldCommitAttacker",
            pose,
            PlayerPersistedState::new_default(pose),
        );
        let mut target_state = PlayerPersistedState::new_default(pose);
        let slot = 45;
        let initial_stack = ItemStack::new(1, 1);
        target_state.inventory.slots[slot] = initial_stack.clone();
        let target = register_player(&registry, "ShieldCommitTarget", pose, target_state.clone());
        let initial_shield = ActiveShield {
            started_tick: 10,
            slot,
            expected_stack: initial_stack,
        };
        registry.set_active_shield(target, Some(initial_shield.clone()));

        let refreshed_stack = ItemStack::new(1, 1).with_damage(5);
        let mut refreshed_inventory = target_state.inventory.clone();
        refreshed_inventory.slots[slot] = refreshed_stack.clone();
        let refresh_plan = survival_plan(&target_state, refreshed_inventory, pose);
        let refreshed_shield = ActiveShield {
            expected_stack: refreshed_stack.clone(),
            ..initial_shield.clone()
        };
        let refreshed = registry.commit_player_attack(
            &SimulationAuthority::for_test(),
            PlayerAttackCommit {
                attacker_session: attacker,
                target_session: target,
                expected_attacker_mode: GameMode::Survival,
                attacker_costs: None,
                expected_shield: Some(initial_shield),
                shield_after_block: Some(ShieldAfterBlock::Refresh(refreshed_shield.clone())),
                next_resistance: None,
                target_plan: &refresh_plan,
            },
        );
        assert!(refreshed.is_some());
        assert_eq!(
            registry
                .lock_inner("verify refreshed shield identity")
                .active_shields
                .get(&target),
            Some(&refreshed_shield),
        );

        target_state.inventory = refresh_plan.updated_inventory.clone();
        let mut broken_inventory = target_state.inventory.clone();
        broken_inventory.slots[slot] = ItemStack::EMPTY;
        let break_plan = survival_plan(&target_state, broken_inventory, pose);
        let broken = registry.commit_player_attack(
            &SimulationAuthority::for_test(),
            PlayerAttackCommit {
                attacker_session: attacker,
                target_session: target,
                expected_attacker_mode: GameMode::Survival,
                attacker_costs: None,
                expected_shield: Some(refreshed_shield),
                shield_after_block: Some(ShieldAfterBlock::Remove),
                next_resistance: None,
                target_plan: &break_plan,
            },
        );
        assert!(broken.is_some());
        assert!(
            !registry
                .lock_inner("verify broken shield removal")
                .active_shields
                .contains_key(&target)
        );
    }

    #[test]
    fn pvp_damage_defers_publication_until_the_sleeping_bed_is_released() {
        let registry = SessionRegistry::new();
        let attacker_pose = PlayerPose::new(0.5, 64.0, 0.5);
        let target_pose = PlayerPose::new(1.0, 64.0, 0.5);
        let attacker = register_player(
            &registry,
            "SleepingPvpAttacker",
            attacker_pose,
            PlayerPersistedState::new_default(attacker_pose),
        );
        let target = register_player(
            &registry,
            "SleepingPvpTarget",
            target_pose,
            PlayerPersistedState::new_default(target_pose),
        );
        let target_entity = {
            let inner = registry.lock_inner("read PvP target entity id");
            EntityId(inner.sessions[&target].entity_id)
        };
        let bed = mc_world::BlockPos { x: 1, y: 64, z: 0 };
        registry.set_world_time(13_000);
        assert!(matches!(
            registry.begin_sleep_at(target, bed),
            crate::play::session::SleepOutcome::Waiting { .. }
        ));

        let result = registry.player_attack_entity(
            &SimulationAuthority::for_test(),
            PlayerEntityAttack {
                attacker_session: attacker,
                entity_id: target_entity,
                amount: 2.0,
                attacker_costs: None,
                authority_tick: registry.simulation_tick(),
            },
        );
        let PlayerAttackResult::Damaged(outcome) = result else {
            panic!("in-range PvP damage must commit");
        };
        let EntityAttackOutcome::PlayerDamaged { dispatches, .. } = *outcome else {
            panic!("player target must use player damage publication");
        };
        assert!(matches!(
            dispatches.as_slice(),
            [VisibilityDispatch {
                command: OutboundCommand::WakeFromBed { bed: wake_bed },
                ..
            }] if *wake_bed == bed
        ));

        let token = registry
            .claim_sleep_wake(target, bed)
            .expect("committed damage must stage an exact wake token");
        let completed = registry
            .complete_sleep_wake(token)
            .expect("confirmed bed release completes the staged wake");
        assert!(completed.dispatches.iter().any(|dispatch| matches!(
            &dispatch.command,
            OutboundCommand::PlayerDamageCommitted { .. }
        )));
    }

    #[test]
    fn pvp_hurt_resistance_uses_the_supplied_authority_tick() {
        let registry = SessionRegistry::new();
        let attacker_pose = PlayerPose::new(0.5, 64.0, 0.5);
        let target_pose = PlayerPose::new(1.0, 64.0, 0.5);
        let attacker = register_player(
            &registry,
            "AuthorityTickAttacker",
            attacker_pose,
            PlayerPersistedState::new_default(attacker_pose),
        );
        let target = register_player(
            &registry,
            "AuthorityTickTarget",
            target_pose,
            PlayerPersistedState::new_default(target_pose),
        );
        let (target_entity, target_state) = {
            let inner = registry.lock_inner("read authority tick target");
            (
                EntityId(inner.sessions[&target].entity_id),
                Arc::clone(&inner.player_persistence[&target]),
            )
        };

        for authority_tick in [100, 110] {
            assert!(matches!(
                registry.player_attack_entity(
                    &SimulationAuthority::for_test(),
                    PlayerEntityAttack {
                        attacker_session: attacker,
                        entity_id: target_entity,
                        amount: 1.0,
                        attacker_costs: None,
                        authority_tick,
                    },
                ),
                PlayerAttackResult::Damaged(_)
            ));
        }

        let health = target_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .survival
            .health;
        assert_eq!(health, 18.0);
        assert_eq!(registry.simulation_tick(), 0);
    }

    #[test]
    fn projectile_damage_defers_publication_until_the_sleeping_bed_is_released() {
        let registry = SessionRegistry::new();
        let target_pose = PlayerPose::new(1.0, 64.0, 0.5);
        let target_state = PlayerPersistedState::new_default(target_pose);
        let target = register_player(
            &registry,
            "SleepingProjectileTarget",
            target_pose,
            target_state.clone(),
        );
        let bed = mc_world::BlockPos { x: 1, y: 64, z: 0 };
        registry.set_world_time(13_000);
        assert!(matches!(
            registry.begin_sleep_at(target, bed),
            crate::play::session::SleepOutcome::Waiting { .. }
        ));

        let mut target_plan =
            survival_plan(&target_state, target_state.inventory.clone(), target_pose);
        target_plan.updated_survival.apply_damage(2.0);
        let prepared = PreparedProjectilePlayerDamage {
            target_session: target,
            expected_shield: None,
            shield_after_block: None,
            next_resistance: None,
            target_plan,
            damage_applied: true,
            fresh_hurt: true,
            shield_blocked: false,
            source_origin: Some(mc_entity::Vec3::new(0.5, 64.0, 0.5)),
        };
        let mut dispatches = Vec::new();
        let mut inner = registry.lock_session_entities("commit projectile damage test");
        assert!(commit_projectile_player_damage_locked(
            &mut inner,
            prepared,
            |_| true,
            &mut dispatches,
        ));
        drop(inner);

        assert!(matches!(
            dispatches.as_slice(),
            [VisibilityDispatch {
                command: OutboundCommand::WakeFromBed { bed: wake_bed },
                ..
            }] if *wake_bed == bed
        ));
        let token = registry
            .claim_sleep_wake(target, bed)
            .expect("committed projectile damage must stage an exact wake token");
        let completed = registry
            .complete_sleep_wake(token)
            .expect("confirmed bed release completes projectile damage wake");
        assert!(completed.dispatches.iter().any(|dispatch| matches!(
            &dispatch.command,
            OutboundCommand::PlayerDamageCommitted { .. }
        )));
    }
}
