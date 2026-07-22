use super::script_commit_events::push_player_death_event_locked;
use super::sleep::SleepWakeReason;
use super::{
    PlayerInventoryCommitError, SessionEntityGuards, SessionId, SessionRegistry,
    spawn_item_drop_locked, spawn_xp_orb_locked,
};
use crate::play::combat::ActiveShield;
use crate::play::persistence::PlayerPersistedState;
use crate::play::recoverable_death_xp;
use crate::play::simulation::{
    AuthoritativePlayerStateSnapshot, CommittedPlayerSurvival, PlayerSurvivalCommitOutcome,
    PlayerSurvivalPlan, SimulationAuthority,
};
use crate::play::{ContainerPlayerPlan, PlayerInventoryCommitOutcome};
use mc_entity::{EntityItemStack, Vec3};
use mc_protocol::packets::play::{GameMode, ItemStack};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tracing::warn;

impl SessionRegistry {
    pub(in crate::play) fn register_player_persistence(
        &self,
        id: SessionId,
        state: Arc<Mutex<PlayerPersistedState>>,
    ) {
        let mut inner = self.lock_inner("register player persistence");
        let (game_mode, dead) = {
            let state = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (state.game_mode, state.survival.is_dead())
        };
        inner.player_persistence.insert(id, state);
        if let Some(uuid) = inner.sessions.get(&id).map(|session| session.uuid) {
            inner.disconnected_player_persistence.remove(&uuid);
        }
        if game_mode == GameMode::Spectator {
            inner.spectator_sessions.insert(id);
        } else {
            inner.spectator_sessions.remove(&id);
        }
        if dead {
            inner.dead_sessions.insert(id);
        } else {
            inner.dead_sessions.remove(&id);
        }
        inner.publish_combat_target(id);
        let became_no_live_sessions = self.publish_live_session_count(&inner);
        drop(inner);
        if became_no_live_sessions {
            self.reconcile_hostile_targets_after_live_session_change();
        }
    }

    pub(in crate::play) fn set_active_shield(&self, id: SessionId, shield: Option<ActiveShield>) {
        let mut inner = self.lock_inner("publish active shield state");
        if !inner.sessions.contains_key(&id) {
            return;
        }
        if let Some(shield) = shield {
            inner.active_shields.insert(id, shield);
        } else {
            inner.active_shields.remove(&id);
        }
    }

    pub(in crate::play) fn recoverable_player_state(
        &self,
        uuid: uuid::Uuid,
    ) -> Option<PlayerPersistedState> {
        let state = {
            let inner = self.lock_inner("recover disconnected player persistence");
            Arc::clone(&inner.disconnected_player_persistence.get(&uuid)?.state)
        };
        let state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "recover disconnected player persistence snapshot",
            Instant::now(),
            state.lock().unwrap_or_else(|poisoned| {
                warn!(
                    player = %uuid,
                    "player persistence mutex was poisoned during reconnect; recovering state"
                );
                poisoned.into_inner()
            }),
        );
        Some((*state).clone())
    }

    pub(in crate::play) fn commit_player_inventory(
        &self,
        _authority: &SimulationAuthority,
        actor_session: SessionId,
        player: &ContainerPlayerPlan,
    ) -> Result<PlayerInventoryCommitOutcome, PlayerInventoryCommitError> {
        let mut inner = self.lock_session_entities("commit player inventory");
        if !inner.sessions.contains_key(&actor_session) {
            return Err(PlayerInventoryCommitError::MissingPlayer);
        }
        let Some(player_state) = inner.player_persistence.get(&actor_session).cloned() else {
            return Err(PlayerInventoryCommitError::MissingPlayer);
        };
        let wait_started = Instant::now();
        let guard = player_state.lock().unwrap_or_else(|poisoned| {
            warn!(
                session_id = actor_session,
                "player persistence mutex was poisoned during inventory commit; recovering state"
            );
            poisoned.into_inner()
        });
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit player inventory state",
            wait_started,
            guard,
        );
        if player_state.inventory.slots != player.expected_inventory.slots
            || player_state.carried_item != player.expected_carried_item
            || player
                .crafting_table_input
                .as_ref()
                .is_some_and(|plan| player_state.crafting_table_input != plan.expected)
            || player
                .enchanting_table_input
                .as_ref()
                .is_some_and(|plan| player_state.enchanting_table_input != plan.expected)
        {
            return Ok(PlayerInventoryCommitOutcome::Rejected {
                inventory: player_state.inventory.clone(),
                carried_item: player_state.carried_item.clone(),
                crafting_table_input: player_state.crafting_table_input.clone(),
                enchanting_table_input: player_state.enchanting_table_input.clone(),
            });
        }

        player_state.replace_container(
            player.updated_inventory.clone(),
            player.updated_carried_item.clone(),
        );
        if let Some(plan) = &player.crafting_table_input {
            player_state.crafting_table_input = plan.updated.clone();
        }
        if let Some(plan) = &player.enchanting_table_input {
            player_state.enchanting_table_input = plan.updated.clone();
        }
        let mut dispatches = Vec::new();
        for drop in &player.drops {
            dispatches.extend(spawn_item_drop_locked(
                &mut inner,
                drop.entity_type_id,
                drop.position,
                drop.stack.clone(),
            ));
        }
        Ok(PlayerInventoryCommitOutcome::Committed {
            inventory: player_state.inventory.clone(),
            carried_item: player_state.carried_item.clone(),
            crafting_table_input: player_state.crafting_table_input.clone(),
            enchanting_table_input: player_state.enchanting_table_input.clone(),
            dispatches,
        })
    }

    pub(crate) fn persisted_player_states(
        &self,
    ) -> Vec<(uuid::Uuid, PlayerPersistedState, Option<u64>)> {
        let entries = {
            let inner = self.lock_inner("save-all player persistence entries");
            let mut entries = inner
                .disconnected_player_persistence
                .iter()
                .map(|(uuid, pending)| {
                    (*uuid, Some(pending.generation), Arc::clone(&pending.state))
                })
                .collect::<Vec<_>>();
            entries.extend(inner.sessions.iter().filter_map(|(id, session)| {
                inner
                    .player_persistence
                    .get(id)
                    .map(|state| (session.uuid, None, Arc::clone(state)))
            }));
            entries
        };
        entries
            .into_iter()
            .map(|(uuid, disconnected_generation, state)| {
                let state = crate::lock_metrics::timed_guard(
                    crate::lock_metrics::LockMetricKind::PlayerPersistence,
                    "save-all player persistence snapshot",
                    Instant::now(),
                    state.lock().unwrap_or_else(|poisoned| {
                        warn!(
                            "player persistence mutex was poisoned during save-all; recovering state"
                        );
                        poisoned.into_inner()
                    }),
                );
                (uuid, (*state).clone(), disconnected_generation)
            })
            .collect()
    }

    pub(crate) fn acknowledge_saved_player_states(&self, saved: &[(uuid::Uuid, u64)]) {
        if saved.is_empty() {
            return;
        }
        let mut inner = self.lock_inner("acknowledge saved player persistence");
        for &(uuid, generation) in saved {
            if inner
                .disconnected_player_persistence
                .get(&uuid)
                .is_some_and(|pending| pending.generation == generation)
            {
                inner.disconnected_player_persistence.remove(&uuid);
            }
        }
    }

    pub(crate) fn player_save_generation(&self) -> u64 {
        self.player_save_generation.load(Ordering::Acquire)
    }

    pub(crate) async fn wait_for_player_save_request(&self, observed: u64) {
        loop {
            let requested = self.player_save_requested.notified();
            tokio::pin!(requested);
            requested.as_mut().enable();
            if self.player_save_generation() != observed {
                return;
            }
            requested.await;
        }
    }

    pub(super) fn mark_player_save_requested(&self) {
        self.player_save_generation.fetch_add(1, Ordering::Release);
        self.player_save_requested.notify_waiters();
    }

    pub(in crate::play) fn commit_player_survival(
        &self,
        _authority: &SimulationAuthority,
        actor_session: SessionId,
        plan: &PlayerSurvivalPlan,
    ) -> Option<PlayerSurvivalCommitOutcome> {
        let mut inner = self.lock_session_entities("commit player survival");
        if !inner.sessions.contains_key(&actor_session) {
            return None;
        }
        let player_state = inner.player_persistence.get(&actor_session)?.clone();
        let wait_started = Instant::now();
        let guard = player_state.lock().unwrap_or_else(|poisoned| {
            warn!(
                session_id = actor_session,
                "player persistence mutex was poisoned during survival transition; recovering state"
            );
            poisoned.into_inner()
        });
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit player survival",
            wait_started,
            guard,
        );
        let active_shield = inner.active_shields.get(&actor_session).cloned();
        if !player_survival_plan_matches(&player_state, plan)
            || plan
                .active_shield
                .as_ref()
                .is_some_and(|transition| transition.expected != active_shield)
        {
            return Some(PlayerSurvivalCommitOutcome::Rejected(
                AuthoritativePlayerStateSnapshot {
                    inventory: player_state.inventory.clone(),
                    carried_item: player_state.carried_item.clone(),
                    active_shield,
                },
            ));
        }
        let respawned = plan.expected_survival.is_dead() && !plan.updated_survival.is_dead();
        let staged_damage_wake = (plan.updated_survival.health < plan.expected_survival.health)
            .then(|| {
                self.stage_sleep_wake_locked(&mut inner, actor_session, SleepWakeReason::Damage)
            })
            .flatten();
        let mut committed =
            apply_player_survival_plan_locked(&mut inner, actor_session, &mut player_state, plan);
        let became_no_live_sessions = self.publish_live_session_count(&inner);
        if let Some(transition) = &plan.active_shield {
            if let Some(shield) = &transition.updated {
                inner.active_shields.insert(actor_session, shield.clone());
            } else {
                inner.active_shields.remove(&actor_session);
            }
        }
        if respawned {
            inner.player_hurt_resistance.remove(&actor_session);
        }
        drop(player_state);
        drop(inner);
        if became_no_live_sessions {
            self.reconcile_hostile_targets_after_live_session_change();
        }
        self.append_spawned_xp_pickup_candidates(&mut committed.dispatches);
        if let Some(sleeper) = staged_damage_wake {
            self.defer_staged_sleep_dispatches(actor_session, &mut committed.dispatches);
            let mut dispatches = self.completed_sleep_dispatches(vec![sleeper], None);
            dispatches.append(&mut committed.dispatches);
            committed.dispatches = dispatches;
        }
        Some(PlayerSurvivalCommitOutcome::Committed(committed))
    }

    #[cfg(test)]
    pub(in crate::play) fn mark_player_dead_for_test(&self, id: SessionId) {
        let became_no_live_sessions = {
            let mut inner = self.lock_inner("mark test player dead");
            inner.dead_sessions.insert(id);
            inner.publish_combat_target(id);
            self.publish_live_session_count(&inner)
        };
        if became_no_live_sessions {
            self.reconcile_hostile_targets_after_live_session_change();
        }
    }
}

pub(super) fn player_survival_plan_matches(
    state: &PlayerPersistedState,
    plan: &PlayerSurvivalPlan,
) -> bool {
    state.survival == plan.expected_survival
        && state.inventory.slots == plan.expected_inventory.slots
        && state.carried_item == plan.expected_carried_item
        && state.xp == plan.expected_xp
        && plan
            .enchanting_table_input
            .as_ref()
            .is_none_or(|input| state.enchanting_table_input == input.expected)
}

pub(super) fn player_attack_cost_plan_matches(
    state: &PlayerPersistedState,
    plan: &PlayerSurvivalPlan,
) -> bool {
    state.survival.food == plan.expected_survival.food
        && state.survival.saturation == plan.expected_survival.saturation
        && state.survival.exhaustion == plan.expected_survival.exhaustion
        && state.inventory.slots == plan.expected_inventory.slots
        && state.carried_item == plan.expected_carried_item
        && state.xp == plan.expected_xp
}

pub(super) fn apply_player_survival_plan_locked(
    inner: &mut SessionEntityGuards<'_>,
    actor_session: SessionId,
    player_state: &mut PlayerPersistedState,
    plan: &PlayerSurvivalPlan,
) -> CommittedPlayerSurvival {
    let died = !plan.expected_survival.is_dead() && plan.updated_survival.is_dead();
    let mut inventory = plan.updated_inventory.clone();
    let mut carried_item = plan.expected_carried_item.clone();
    let mut xp = plan.updated_xp.clone();
    let mut dispatches = Vec::new();
    if died {
        let mut drops = Vec::new();
        for slot in &mut inventory.slots[1..] {
            let stack = std::mem::take(slot);
            if !stack.is_empty() {
                drops.push(EntityItemStack {
                    item_id: stack.item_id,
                    count: stack.count,
                    damage: stack.damage,
                    enchantments: stack.enchantments,
                });
            }
        }
        if !carried_item.is_empty() {
            drops.push(EntityItemStack {
                item_id: carried_item.item_id,
                count: carried_item.count,
                damage: carried_item.damage,
                enchantments: carried_item.enchantments.clone(),
            });
            carried_item = ItemStack::EMPTY;
        }
        if let Some(entity_type_id) = plan.item_entity_type_id {
            let position = Vec3::new(plan.position.x, plan.position.y + 1.0, plan.position.z);
            for drop in drops {
                dispatches.extend(spawn_item_drop_locked(
                    inner,
                    entity_type_id,
                    position,
                    drop,
                ));
            }
        }
        let dropped_xp = recoverable_death_xp(&xp);
        if let Some(entity_type_id) = plan.xp_orb_entity_type_id {
            dispatches.extend(spawn_xp_orb_locked(
                inner,
                entity_type_id,
                plan.position,
                dropped_xp,
            ));
        }
        xp.reset();
    }

    player_state.survival = plan.updated_survival;
    player_state.replace_container(inventory.clone(), carried_item.clone());
    player_state.replace_xp(xp.clone());
    if plan.updated_survival.is_dead() {
        inner.dead_sessions.insert(actor_session);
    } else {
        inner.dead_sessions.remove(&actor_session);
    }
    inner.publish_combat_target(actor_session);
    if let Some(input) = &plan.enchanting_table_input {
        player_state.enchanting_table_input = input.updated.clone();
    }
    if died {
        push_player_death_event_locked(inner, actor_session, player_state, plan.position);
    }
    CommittedPlayerSurvival {
        survival: plan.updated_survival,
        inventory,
        carried_item,
        xp,
        died,
        dispatches,
    }
}
