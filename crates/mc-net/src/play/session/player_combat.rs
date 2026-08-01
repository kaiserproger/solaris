use super::interaction_geometry::{player_aabb_for_pose, within_entity_attack_reach};
use super::outbound::{
    OutboundCommand, PlayerCarriedItemDelta, PlayerDamagePublication, PlayerHurtEvent,
    PlayerInventorySlotDelta, PlayerXpDelta, SessionRecipient, ShieldCooldownPublication,
    VisibilityDispatch,
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
    SessionEntityGuards, SessionId, SessionRegistry, session_recipients, visible_observers_locked,
};
use crate::lock_policy::lock_authoritative_mutex;
use crate::play::combat::{
    ActiveShield, PlayerDamageKind, PlayerDamageRequest, PlayerHurtResolution,
    damage_active_shield_slot, melee_knockback, shield_block_knockback, shield_blocks_damage_since,
    shield_disable_ticks, shield_use_matches_slot,
};
use crate::play::inventory::{
    PlayerInventory, damage_inventory_armor, inventory_damage_after_armor,
    inventory_damage_after_protection,
};
use crate::play::simulation::{PlayerSurvivalPlan, SimulationAuthority};
use crate::play::{GameMode, PlayerPose};
use mc_entity::{EntityId, Vec3};
use mc_protocol::packets::play::ItemStack;
use std::sync::Arc;
use std::time::Instant;

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
    shield_disable: Option<ShieldDisablePlan>,
    next_resistance: Option<crate::play::combat::PlayerHurtResistance>,
    target_plan: &'a PlayerSurvivalPlan,
}

enum ShieldAfterBlock {
    Refresh(ActiveShield),
    Remove,
}

#[derive(Clone)]
struct ShieldDisablePlan {
    attacker_slot: usize,
    expected_attacker_stack: ItemStack,
    cooldown_group: mc_data::Identifier,
    duration_ticks: u64,
    deadline_tick: u64,
}

struct AuthoritativeAttackerContext {
    mode: GameMode,
    attack_range: Option<mc_data::item_components::AttackRangeFacts>,
    held_slot: usize,
    held_stack: ItemStack,
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
        if inner.client_unloaded_sessions.contains(&attacker_session) {
            return PlayerAttackResult::ValidationRejected;
        }
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
        if inner.client_unloaded_sessions.contains(&target_session) {
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
        let keep_inventory = inner.keep_inventory;
        drop(inner);

        let Some(attacker_context) = authoritative_attacker_reach(
            attacker_session,
            &attacker_state,
            &combat_resources,
            "validate PvP attacker state",
        ) else {
            return PlayerAttackResult::ValidationRejected;
        };
        let authoritative_mode = attacker_context.mode;
        if !within_entity_attack_reach(
            attacker_pose,
            target_position,
            player_aabb_for_pose(target_pose),
            authoritative_mode,
            attacker_context.attack_range,
        ) {
            return PlayerAttackResult::ValidationRejected;
        }

        let wait_started = Instant::now();
        let target_state = lock_authoritative_mutex(&target_state, "play.player_persistence");
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
        let shield_disable = shield_blocks
            .then(|| {
                let shield = active_shield.as_ref()?;
                let duration_ticks = shield_disable_ticks(
                    items,
                    &combat_resources.item_facts,
                    &attacker_context.held_stack,
                    &shield.expected_stack,
                )?;
                let cooldown_group = items.name_of(shield.expected_stack.item_id)?.clone();
                Some(ShieldDisablePlan {
                    attacker_slot: attacker_context.held_slot,
                    expected_attacker_stack: attacker_context.held_stack.clone(),
                    cooldown_group,
                    duration_ticks,
                    deadline_tick: authority_tick.saturating_add(duration_ticks),
                })
            })
            .flatten();

        let mut updated_inventory = expected.inventory.clone();
        let mut updated_survival = expected.survival;
        let damage_applied;
        let fresh_hurt;
        let next_resistance;
        let shield_after_block;
        if shield_blocks {
            let shield = active_shield.as_ref().expect("shield was checked above");
            let durability_transition = match damage_active_shield_slot(
                items,
                &combat_resources.item_facts,
                &mut updated_inventory.slots,
                shield.slot,
                &shield.expected_stack,
                damage.amount,
            ) {
                Some((_, _, true)) => ShieldAfterBlock::Remove,
                Some((_, updated_stack, false)) => {
                    let mut refreshed = shield.clone();
                    refreshed.expected_stack = updated_stack;
                    ShieldAfterBlock::Refresh(refreshed)
                }
                None => ShieldAfterBlock::Refresh(shield.clone()),
            };
            shield_after_block = Some(if shield_disable.is_some() {
                ShieldAfterBlock::Remove
            } else {
                durability_transition
            });
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
            keep_inventory,
            position: target_position,
        };
        let shield_cooldown = shield_disable.as_ref().and_then(|disable| {
            i32::try_from(disable.duration_ticks)
                .ok()
                .map(|duration| ShieldCooldownPublication {
                    cooldown_group: disable.cooldown_group.clone(),
                    duration,
                })
        });
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
                    shield_disable,
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
            shield_cooldown,
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
            shield_disable,
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
        let first_guard = lock_authoritative_mutex(first, "play.player_persistence");
        let first_guard = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit player attack first participant",
            first_wait,
            first_guard,
        );
        let second_wait = Instant::now();
        let second_guard = lock_authoritative_mutex(second, "play.player_persistence");
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
        if let Some(disable) = &shield_disable
            && (PlayerInventory::HOTBAR_BASE + usize::from(attacker_state.selected_hotbar_slot)
                != disable.attacker_slot
                || attacker_state.inventory.slots.get(disable.attacker_slot)
                    != Some(&disable.expected_attacker_stack))
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
            let committed = apply_player_survival_plan_locked(
                &mut inner,
                attacker_session,
                &mut attacker_state,
                &effective,
            );
            CommittedPlayerAttackCosts {
                survival: committed.survival,
                inventory: committed.inventory,
            }
        });
        let mut committed_target = apply_player_survival_plan_locked(
            &mut inner,
            target_session,
            &mut target_state,
            target_plan,
        );
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
        if let Some(disable) = shield_disable {
            inner.active_shields.remove(&target_session);
            inner
                .shield_disabled_until
                .entry(target_session)
                .and_modify(|deadline| *deadline = (*deadline).max(disable.deadline_tick))
                .or_insert(disable.deadline_tick);
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
    _attacker_session: SessionId,
    attacker_state: &std::sync::Mutex<crate::play::persistence::PlayerPersistedState>,
    operation: &'static str,
) -> Option<GameMode> {
    let wait_started = Instant::now();
    let state = lock_authoritative_mutex(attacker_state, "play.player_persistence");
    let state = crate::lock_metrics::timed_guard(
        crate::lock_metrics::LockMetricKind::PlayerPersistence,
        operation,
        wait_started,
        state,
    );
    (!state.survival.is_dead() && state.game_mode != GameMode::Spectator).then_some(state.game_mode)
}

fn authoritative_attacker_reach(
    _attacker_session: SessionId,
    attacker_state: &std::sync::Mutex<crate::play::persistence::PlayerPersistedState>,
    resources: &super::PlayerCombatResources,
    operation: &'static str,
) -> Option<AuthoritativeAttackerContext> {
    let wait_started = Instant::now();
    let state = lock_authoritative_mutex(attacker_state, "play.player_persistence");
    let state = crate::lock_metrics::timed_guard(
        crate::lock_metrics::LockMetricKind::PlayerPersistence,
        operation,
        wait_started,
        state,
    );
    if state.survival.is_dead() || state.game_mode == GameMode::Spectator {
        return None;
    }
    let held_slot = PlayerInventory::HOTBAR_BASE + usize::from(state.selected_hotbar_slot);
    let held_stack = state.inventory.slots.get(held_slot)?.clone();
    Some(AuthoritativeAttackerContext {
        mode: state.game_mode,
        attack_range: held_attack_range(resources, &state),
        held_slot,
        held_stack,
    })
}

pub(super) fn held_attack_range(
    resources: &super::PlayerCombatResources,
    state: &crate::play::persistence::PlayerPersistedState,
) -> Option<mc_data::item_components::AttackRangeFacts> {
    let held = state.inventory.held(state.selected_hotbar_slot)?;
    let name = resources.items.name_of(held.item_id)?;
    resources.item_facts.get(name)?.attack_range
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
    if inner.client_unloaded_sessions.contains(&target_session) {
        return ProjectilePlayerDamagePreview::Rejected(None);
    }
    let Some(target) = inner.sessions.get(&target_session) else {
        return ProjectilePlayerDamagePreview::Rejected(None);
    };
    let target_pose = target.pose;
    let target_position = Vec3::new(target_pose.x, target_pose.y, target_pose.z);
    let Some(target_state) = inner.player_persistence.get(&target_session).cloned() else {
        return ProjectilePlayerDamagePreview::Rejected(None);
    };
    let wait_started = Instant::now();
    let target_state = lock_authoritative_mutex(&target_state, "play.player_persistence");
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
        keep_inventory: inner.keep_inventory,
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
    let target_state = lock_authoritative_mutex(&target_state, "play.player_persistence");
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
    let mut committed =
        apply_player_survival_plan_locked(inner, target_session, &mut target_state, &target_plan);
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
        shield_cooldown: None,
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
    use mc_script::{ScriptEventKind, ScriptGameMode};
    use tokio::sync::mpsc;

    use super::{
        EntityAttackOutcome, EntityId, OutboundCommand, PlayerAttackCommit, PlayerAttackResult,
        PlayerEntityAttack, PreparedProjectilePlayerDamage, ShieldAfterBlock,
        ShieldCooldownPublication, VisibilityDispatch, commit_projectile_player_damage_locked,
    };
    use crate::login::LoggedInProfile;
    use crate::play::combat::{
        ActiveShield, ShieldUseState, damage_active_shield_slot, damage_active_shield_slots,
        shield_use_matches, shield_use_matches_slot,
    };
    use crate::play::persistence::PlayerPersistedState;
    use crate::play::session::SessionRegistry;
    use crate::play::simulation::{PlayerSurvivalPlan, SimulationAuthority};
    use crate::play::{GameMode, PlayerInventory, PlayerPose, SurvivalState};

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
            keep_inventory: false,
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
                shield_disable: None,
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
                shield_disable: None,
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
    fn frontal_axe_block_disables_shield_with_exact_owner_deadline_and_publication() {
        let shield_name = Identifier::parse("minecraft:shield").unwrap();
        let axe_name = Identifier::parse("minecraft:iron_axe").unwrap();
        let items = Arc::new(ItemRegistry::from_report(&[
            ItemReport {
                id: shield_name.clone(),
                protocol_id: 1,
            },
            ItemReport {
                id: axe_name.clone(),
                protocol_id: 2,
            },
        ]));
        let facts = Arc::new(ItemFactsTable::from_entries([
            (
                shield_name.clone(),
                ItemFacts {
                    max_damage: Some(336),
                    blocks_attacks_disable_cooldown_scale: Some(1.0),
                    ..ItemFacts::default()
                },
            ),
            (
                axe_name,
                ItemFacts {
                    weapon: true,
                    weapon_damage_per_attack: Some(2),
                    weapon_disable_blocking_seconds: Some(5.0),
                    ..ItemFacts::default()
                },
            ),
        ]));
        let registry = SessionRegistry::new();
        registry.configure_player_combat(None, None, Arc::clone(&items), Arc::clone(&facts));

        let attacker_pose = PlayerPose::new(0.5, 64.0, 0.5);
        let mut attacker_state = PlayerPersistedState::new_default(attacker_pose);
        attacker_state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(2, 1);
        let attacker = register_player(&registry, "AxeDisableAtk", attacker_pose, attacker_state);

        let mut target_pose = PlayerPose::new(0.5, 64.0, 1.5);
        target_pose.yaw = 180.0;
        let mut target_state = PlayerPersistedState::new_default(target_pose);
        target_state.inventory.slots[PlayerInventory::OFFHAND_SLOT] = ItemStack::new(1, 1);
        let target = register_player(&registry, "AxeDisableDef", target_pose, target_state);
        registry.set_active_shield(
            target,
            Some(ActiveShield {
                started_tick: 0,
                slot: PlayerInventory::OFFHAND_SLOT,
                expected_stack: ItemStack::new(1, 1),
            }),
        );
        registry.advance_world_time(crate::play::combat::SHIELD_ACTIVATION_DELAY_TICKS);
        let authority_tick = registry.simulation_tick();
        let target_entity = {
            let inner = registry.lock_inner("read axe-disable target entity");
            EntityId(inner.sessions[&target].entity_id)
        };

        let result = registry.player_attack_entity(
            &SimulationAuthority::for_test(),
            PlayerEntityAttack {
                attacker_session: attacker,
                entity_id: target_entity,
                amount: 4.0,
                attacker_costs: None,
                authority_tick,
            },
        );
        let PlayerAttackResult::Damaged(outcome) = result else {
            panic!("front axe hit must reach player shield authority")
        };
        let EntityAttackOutcome::PlayerDamaged {
            dispatches,
            damage_applied,
            ..
        } = *outcome
        else {
            panic!("player target must use player damage publication")
        };
        assert!(!damage_applied, "the disabling axe hit is still blocked");
        let cooldown = dispatches
            .iter()
            .find_map(|dispatch| match &dispatch.command {
                OutboundCommand::PlayerDamageCommitted { publication, .. } => {
                    assert!(publication.shield_blocked);
                    publication.shield_cooldown.clone()
                }
                _ => None,
            });
        assert_eq!(
            cooldown,
            Some(ShieldCooldownPublication {
                cooldown_group: shield_name,
                duration: 100,
            })
        );

        let target_uuid = crate::login::offline_uuid("AxeDisableDef");
        let persisted = registry
            .persisted_player_states()
            .into_iter()
            .find(|(uuid, _, _)| *uuid == target_uuid)
            .map(|(_, state, _)| state)
            .expect("target persistence remains registered");
        assert_eq!(persisted.survival.health, SurvivalState::MAX_HEALTH);
        assert_eq!(
            persisted.inventory.slots[PlayerInventory::OFFHAND_SLOT].damage,
            Some(5),
        );
        {
            let inner = registry.lock_inner("verify axe-disable owner state");
            assert!(!inner.active_shields.contains_key(&target));
            assert_eq!(
                inner.shield_disabled_until.get(&target),
                Some(&authority_tick.saturating_add(100)),
            );
        }
        assert_eq!(
            registry.shield_disable_remaining_ticks(target, authority_tick),
            Some(100)
        );
        assert_eq!(
            registry.shield_disable_remaining_ticks(target, authority_tick + 99),
            Some(1)
        );
        assert_eq!(
            registry.shield_disable_remaining_ticks(target, authority_tick + 100),
            None
        );
    }

    #[test]
    fn stale_axe_identity_rejects_shield_disable_without_target_mutation() {
        let registry = SessionRegistry::new();
        let pose = PlayerPose::new(0.5, 64.0, 0.5);
        let mut attacker_state = PlayerPersistedState::new_default(pose);
        attacker_state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(2, 1);
        let attacker = register_player(&registry, "StaleAxeAtk", pose, attacker_state);
        let mut target_state = PlayerPersistedState::new_default(pose);
        target_state.inventory.slots[PlayerInventory::OFFHAND_SLOT] = ItemStack::new(1, 1);
        let target = register_player(&registry, "StaleAxeDef", pose, target_state.clone());
        let shield = ActiveShield {
            started_tick: 0,
            slot: PlayerInventory::OFFHAND_SLOT,
            expected_stack: ItemStack::new(1, 1),
        };
        registry.set_active_shield(target, Some(shield.clone()));
        let target_plan = survival_plan(&target_state, target_state.inventory.clone(), pose);
        {
            let attacker_state = registry
                .lock_inner("replace stale axe fixture")
                .player_persistence[&attacker]
                .clone();
            attacker_state
                .lock()
                .expect("test lock poisoned")
                .inventory
                .slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(3, 1);
        }

        let committed = registry.commit_player_attack(
            &SimulationAuthority::for_test(),
            PlayerAttackCommit {
                attacker_session: attacker,
                target_session: target,
                expected_attacker_mode: GameMode::Survival,
                attacker_costs: None,
                expected_shield: Some(shield.clone()),
                shield_after_block: Some(ShieldAfterBlock::Remove),
                shield_disable: Some(super::ShieldDisablePlan {
                    attacker_slot: PlayerInventory::HOTBAR_BASE,
                    expected_attacker_stack: ItemStack::new(2, 1),
                    cooldown_group: Identifier::parse("minecraft:shield").unwrap(),
                    duration_ticks: 100,
                    deadline_tick: 100,
                }),
                next_resistance: None,
                target_plan: &target_plan,
            },
        );
        assert!(committed.is_none());
        {
            let attacker_state = registry
                .lock_inner("switch stale axe selected slot")
                .player_persistence[&attacker]
                .clone();
            let mut attacker_state = attacker_state.lock().expect("test lock poisoned");
            attacker_state.inventory.slots[PlayerInventory::HOTBAR_BASE] = ItemStack::new(2, 1);
            attacker_state.inventory.slots[PlayerInventory::HOTBAR_BASE + 1] = ItemStack::new(3, 1);
            attacker_state.selected_hotbar_slot = 1;
        }
        let selected_slot_stale = registry.commit_player_attack(
            &SimulationAuthority::for_test(),
            PlayerAttackCommit {
                attacker_session: attacker,
                target_session: target,
                expected_attacker_mode: GameMode::Survival,
                attacker_costs: None,
                expected_shield: Some(shield.clone()),
                shield_after_block: Some(ShieldAfterBlock::Remove),
                shield_disable: Some(super::ShieldDisablePlan {
                    attacker_slot: PlayerInventory::HOTBAR_BASE,
                    expected_attacker_stack: ItemStack::new(2, 1),
                    cooldown_group: Identifier::parse("minecraft:shield").unwrap(),
                    duration_ticks: 100,
                    deadline_tick: 100,
                }),
                next_resistance: None,
                target_plan: &target_plan,
            },
        );
        assert!(selected_slot_stale.is_none());

        let inner = registry.lock_inner("verify stale axe rejection");
        assert_eq!(inner.active_shields.get(&target), Some(&shield));
        assert!(!inner.shield_disabled_until.contains_key(&target));
        let target_state = inner.player_persistence[&target]
            .lock()
            .expect("test lock poisoned");
        assert_eq!(
            target_state.inventory.slots,
            target_plan.expected_inventory.slots
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
    fn client_unloaded_after_respawn_cannot_attack() {
        let registry = SessionRegistry::new();
        let attacker_pose = PlayerPose::new(0.5, 64.0, 0.5);
        let target_pose = PlayerPose::new(1.0, 64.0, 0.5);
        let attacker = register_player(
            &registry,
            "UnloadedAttacker",
            attacker_pose,
            PlayerPersistedState::new_default(attacker_pose),
        );
        let target = register_player(
            &registry,
            "LoadedTarget",
            target_pose,
            PlayerPersistedState::new_default(target_pose),
        );
        let target_entity = {
            let mut inner = registry.lock_inner("mark attacker client unloaded");
            inner.client_unloaded_sessions.insert(attacker);
            EntityId(inner.sessions[&target].entity_id)
        };

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

        assert!(matches!(result, PlayerAttackResult::ValidationRejected));
    }

    #[test]
    fn lethal_pvp_pushes_death_even_when_target_outbound_is_closed() {
        let registry = SessionRegistry::new();
        let mut deaths = registry.install_script_commit_event_outbox();
        let attacker_pose = PlayerPose::new(0.5, 64.0, 0.5);
        let target_pose = PlayerPose::new(1.0, 64.0, 0.5);
        let attacker = register_player(
            &registry,
            "PvpDeathAttacker",
            attacker_pose,
            PlayerPersistedState::new_default(attacker_pose),
        );
        let target = register_player(
            &registry,
            "PvpDeathTarget",
            target_pose,
            PlayerPersistedState::new_default(target_pose),
        );
        let target_entity = {
            let inner = registry.lock_inner("read lethal PvP target entity id");
            EntityId(inner.sessions[&target].entity_id)
        };

        let result = registry.player_attack_entity(
            &SimulationAuthority::for_test(),
            PlayerEntityAttack {
                attacker_session: attacker,
                entity_id: target_entity,
                amount: SurvivalState::MAX_HEALTH,
                attacker_costs: None,
                authority_tick: registry.simulation_tick(),
            },
        );
        assert!(matches!(result, PlayerAttackResult::Damaged(_)));
        let event = deaths
            .try_recv_required()
            .expect("PvP owner commit must not depend on target outbound");
        assert!(matches!(
            event.kind(),
            ScriptEventKind::PlayerDied {
                player_id,
                context,
                game_mode: ScriptGameMode::Survival,
                ..
            } if player_id.value() == target && context.username() == "PvpDeathTarget"
        ));
        assert!(matches!(
            deaths.try_recv_required(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn lethal_projectile_pushes_death_from_the_same_owner_commit() {
        let registry = SessionRegistry::new();
        let mut deaths = registry.install_script_commit_event_outbox();
        let target_pose = PlayerPose::new(1.0, 64.0, 0.5);
        let target_state = PlayerPersistedState::new_default(target_pose);
        let target = register_player(&registry, "ArrowDeath", target_pose, target_state.clone());
        let mut target_plan =
            survival_plan(&target_state, target_state.inventory.clone(), target_pose);
        target_plan
            .updated_survival
            .apply_damage(SurvivalState::MAX_HEALTH);
        let prepared = PreparedProjectilePlayerDamage {
            target_session: target,
            expected_shield: None,
            shield_after_block: None,
            next_resistance: None,
            target_plan,
            damage_applied: true,
            fresh_hurt: true,
            shield_blocked: false,
            source_origin: None,
        };
        let mut dispatches = Vec::new();
        let mut inner = registry.lock_session_entities("commit lethal projectile damage test");
        assert!(commit_projectile_player_damage_locked(
            &mut inner,
            prepared,
            |_| true,
            &mut dispatches,
        ));
        drop(inner);

        assert!(matches!(
            deaths.try_recv_required().unwrap().kind(),
            ScriptEventKind::PlayerDied {
                player_id,
                context,
                game_mode: ScriptGameMode::Survival,
                ..
            } if player_id.value() == target && context.username() == "ArrowDeath"
        ));
        assert!(matches!(
            deaths.try_recv_required(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
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
            .expect("test lock poisoned")
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
