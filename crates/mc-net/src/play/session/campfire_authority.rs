use std::collections::HashMap;
use std::sync::{Arc, MutexGuard};
use std::time::Instant;

use mc_protocol::packets::play::ItemStack;
use mc_world::{BlockPos, WorldStorage};
use tracing::warn;

use crate::play::CommittedCampfireCookingTick;
use crate::play::campfire::{CampfireCookingState, PendingCampfireOutput};
use crate::play::simulation::{
    CampfireUsePlan, CommittedCampfireUse, SimulationAuthority, SimulationRequestError,
};

use super::transactions::CampfireUseTransaction;
use super::{SessionId, SessionRegistry};

#[cfg(test)]
#[derive(Debug)]
pub(super) struct CampfireRecoveryProbe {
    reached: tokio::sync::oneshot::Sender<()>,
    resume: tokio::sync::oneshot::Receiver<()>,
}

impl SessionRegistry {
    #[cfg(test)]
    pub(crate) fn install_campfire_d1_probe_for_test(
        &self,
        reached: tokio::sync::oneshot::Sender<()>,
        resume: tokio::sync::oneshot::Receiver<()>,
    ) {
        *self
            .campfire_d1_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(CampfireRecoveryProbe { reached, resume });
    }

    #[cfg(test)]
    pub(crate) fn install_campfire_entity_probe_for_test(
        &self,
        reached: tokio::sync::oneshot::Sender<()>,
        resume: tokio::sync::oneshot::Receiver<()>,
    ) {
        *self
            .campfire_entity_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(CampfireRecoveryProbe { reached, resume });
    }

    #[cfg(test)]
    pub(crate) async fn pause_after_campfire_d1_for_test(&self) {
        let probe = self
            .campfire_d1_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            let _ = probe.reached.send(());
            let _ = probe.resume.await;
        }
    }

    #[cfg(test)]
    pub(crate) async fn pause_after_campfire_entity_commit_for_test(&self) {
        let probe = self
            .campfire_entity_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            let _ = probe.reached.send(());
            let _ = probe.resume.await;
        }
    }

    pub(in crate::play) fn commit_campfire_use(
        &self,
        _authority: &SimulationAuthority,
        storage: &mut WorldStorage,
        actor_session: SessionId,
        plan: &CampfireUsePlan,
    ) -> Result<Option<CommittedCampfireUse>, SimulationRequestError> {
        if storage.get_cached_block(plan.position) != Some(plan.expected_state)
            || storage.block_mutation_token(plan.position) != Some(plan.expected_token)
        {
            return Ok(None);
        }

        let inner = self.lock_inner("commit campfire use");
        if !inner.sessions.contains_key(&actor_session) {
            return Ok(None);
        }
        let Some(player_state) = inner.player_persistence.get(&actor_session).cloned() else {
            return Ok(None);
        };
        let wait_started = Instant::now();
        let guard = player_state.lock().unwrap_or_else(|poisoned| {
            warn!(
                session_id = actor_session,
                "player persistence mutex was poisoned during campfire use; recovering state"
            );
            poisoned.into_inner()
        });
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit campfire use",
            wait_started,
            guard,
        );
        if player_state.inventory.slots[plan.held_slot] != plan.expected_held {
            return Ok(None);
        }

        let mut inventory = player_state.inventory.clone();
        let held = &mut inventory.slots[plan.held_slot];
        held.count = held.count.saturating_sub(1);
        if held.count <= 0 {
            *held = ItemStack::EMPTY;
        }
        let changed_slots = vec![(plan.held_slot, held.clone())];

        let mut campfires = self.lock_campfire_cooking();
        let authoritative = campfires.get(&plan.position).cloned().unwrap_or_default();
        if authoritative != plan.expected_cooking {
            return Ok(None);
        }
        match storage.set_opaque_block_entity(plan.position, plan.persistent_bytes.clone()) {
            Ok(true) => {}
            Ok(false) => return Err(SimulationRequestError::WorldMutationFailed),
            Err(error) => {
                warn!(%error, position = ?plan.position, "campfire use persistence failed");
                return Err(SimulationRequestError::WorldMutationFailed);
            }
        }
        if plan.updated_cooking.is_empty() {
            campfires.remove(&plan.position);
        } else {
            campfires.insert(plan.position, plan.updated_cooking.clone());
        }
        player_state.replace_inventory(inventory.clone());
        Ok(Some(CommittedCampfireUse {
            inventory,
            changed_slots,
        }))
    }

    #[cfg(test)]
    pub(in crate::play) fn insert_campfire_cooking(
        &self,
        position: BlockPos,
        input: ItemStack,
        result: ItemStack,
        cooking_time: u32,
    ) -> Option<CampfireCookingState> {
        self.commit_campfire_cooking_insert(position, input, result, cooking_time, |_| true)
    }

    pub(in crate::play) fn commit_campfire_cooking_insert(
        &self,
        position: BlockPos,
        input: ItemStack,
        result: ItemStack,
        cooking_time: u32,
        commit: impl FnOnce(&CampfireCookingState) -> bool,
    ) -> Option<CampfireCookingState> {
        let mut campfires = self.lock_campfire_cooking();
        let mut cooking = campfires.get(&position).cloned().unwrap_or_default();
        if !cooking.insert(input, result, cooking_time) || !commit(&cooking) {
            return None;
        }
        campfires.insert(position, cooking.clone());
        Some(cooking)
    }

    pub(in crate::play) fn campfire_cooking_state(
        &self,
        position: BlockPos,
    ) -> CampfireCookingState {
        let campfires = self.lock_campfire_cooking();
        campfires.get(&position).cloned().unwrap_or_default()
    }

    #[cfg(test)]
    pub(in crate::play) fn commit_campfire_cooking_legacy_for_test<E>(
        &self,
        position: BlockPos,
        expected: &CampfireCookingState,
        updated: CampfireCookingState,
        commit: impl FnOnce() -> Result<(), E>,
    ) -> Result<bool, E> {
        self.commit_campfire_cooking_inner(position, expected, updated, commit)
    }

    #[cfg(test)]
    fn commit_campfire_cooking_inner<E>(
        &self,
        position: BlockPos,
        expected: &CampfireCookingState,
        updated: CampfireCookingState,
        commit: impl FnOnce() -> Result<(), E>,
    ) -> Result<bool, E> {
        let mut campfires = self.lock_campfire_cooking();
        let authoritative = campfires.get(&position).cloned().unwrap_or_default();
        if &authoritative != expected {
            return Ok(false);
        }
        commit()?;
        if updated.is_empty() {
            campfires.remove(&position);
        } else {
            campfires.insert(position, updated);
        }
        Ok(true)
    }

    pub(in crate::play) fn campfire_cooking_positions(&self) -> Vec<BlockPos> {
        let campfires = self.lock_campfire_cooking();
        campfires.keys().copied().collect()
    }

    pub(in crate::play) fn tick_campfire_cooking_conditionally(
        &self,
        position: BlockPos,
        world_decision_id: u64,
        commit: impl FnOnce(&CampfireCookingState) -> bool,
    ) -> Option<CommittedCampfireCookingTick> {
        let mut campfires = self.lock_campfire_cooking();
        let mut cooking = campfires.get(&position)?.clone();
        let tick = cooking.tick_for_decision(world_decision_id, position);
        if !tick.dirty || !commit(&cooking) {
            return None;
        }
        if cooking.is_empty() {
            campfires.remove(&position);
        } else {
            campfires.insert(position, cooking.clone());
        }
        Some(CommittedCampfireCookingTick {
            cooking,
            completed: tick.completed,
            changed: tick.changed,
        })
    }

    pub(in crate::play) fn acknowledge_pending_campfire_outputs_conditionally(
        &self,
        position: BlockPos,
        expected: &[PendingCampfireOutput],
        commit: impl FnOnce(&CampfireCookingState) -> bool,
    ) -> Option<CampfireCookingState> {
        let mut campfires = self.lock_campfire_cooking();
        let mut cooking = campfires.get(&position)?.clone();
        if cooking.pending_outputs != expected {
            return None;
        }
        cooking.pending_outputs.clear();
        if !commit(&cooking) {
            return None;
        }
        if cooking.is_empty() {
            campfires.remove(&position);
        } else {
            campfires.insert(position, cooking.clone());
        }
        Some(cooking)
    }

    pub(in crate::play) fn cool_down_campfire_cooking_conditionally(
        &self,
        position: BlockPos,
        commit: impl FnOnce(&CampfireCookingState) -> bool,
    ) -> bool {
        let mut campfires = self.lock_campfire_cooking();
        let Some(mut cooking) = campfires.get(&position).cloned() else {
            return false;
        };
        if !cooking.cool_down() || !commit(&cooking) {
            return false;
        }
        campfires.insert(position, cooking);
        true
    }

    pub(in crate::play) fn restore_campfire_cooking(
        &self,
        position: BlockPos,
        cooking: CampfireCookingState,
    ) -> bool {
        if cooking.is_empty() {
            return false;
        }
        let mut campfires = self.lock_campfire_cooking();
        match campfires.entry(position) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(cooking);
                true
            }
            std::collections::hash_map::Entry::Occupied(_) => false,
        }
    }

    pub(in crate::play) fn clear_campfire_cooking(&self, position: BlockPos) -> bool {
        let mut campfires = self.lock_campfire_cooking();
        campfires.remove(&position).is_some()
    }

    pub(in crate::play) fn prepare_campfire_use_transaction(
        &self,
        actor_session: SessionId,
    ) -> Option<CampfireUseTransaction> {
        let inner = self.lock_inner("prepare regional campfire use");
        if !inner.sessions.contains_key(&actor_session) {
            return None;
        }
        inner
            .player_persistence
            .get(&actor_session)
            .cloned()
            .map(|player_state| CampfireUseTransaction {
                player_state,
                campfire_cooking: Arc::clone(&self.campfire_cooking),
            })
    }

    fn lock_campfire_cooking(&self) -> MutexGuard<'_, HashMap<BlockPos, CampfireCookingState>> {
        self.campfire_cooking.lock().unwrap_or_else(|poisoned| {
            warn!("campfire cooking state was poisoned; recovering state");
            poisoned.into_inner()
        })
    }
}
