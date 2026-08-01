use std::sync::Arc;
use std::time::Instant;

use mc_data::block_light::BlockLightTable;
use mc_protocol::packets::play::ItemStack;
use tracing::warn;

use crate::play::block_edit_commit::{
    apply_block_edit_batch_to_storage_conditionally,
    apply_block_edit_batch_with_scheduled_ticks_to_storage_conditionally,
};
use crate::play::bucket_interactions::plan_bucket_replacement;
use crate::play::inventory::PlayerInventory;
use crate::play::simulation::{
    BucketUsePlan, CommittedBucketUse, CommittedSurvivalBreak, CommittedSurvivalPlacement,
    SimulationAuthority, SimulationRequestError, SurvivalBreakPlan, SurvivalPlacementPlan,
    placement_inventory_debit,
};
use crate::play::{BlockEdit, BlockEditBatchOutcome, BlockEditPrecondition};

use super::transactions::{
    BucketUseTransaction, SurvivalBreakTransaction, SurvivalPlacementTransaction,
};
use super::{SessionId, SessionRegistry};

fn rollback_survival_break(
    storage: &mut mc_world::WorldStorage,
    block_light: Option<&BlockLightTable>,
    committed: &BlockEditBatchOutcome,
) -> Result<(), SimulationRequestError> {
    let mut edits = Vec::with_capacity(committed.applied.len());
    let mut preconditions = Vec::with_capacity(committed.applied.len());
    for applied in committed.applied.iter().rev() {
        let Some(expected_token) = committed.resulting_tokens.get(&applied.pos).copied() else {
            warn!(pos = ?applied.pos, "survival break rollback is missing the committed mutation token");
            return Err(SimulationRequestError::WorldMutationFailed);
        };
        edits.push(BlockEdit {
            pos: applied.pos,
            new_state: applied.previous,
        });
        preconditions.push(BlockEditPrecondition {
            pos: applied.pos,
            expected_state: applied.new_state,
            expected_token,
        });
    }
    let Some(rollback) = apply_block_edit_batch_to_storage_conditionally(
        storage,
        block_light,
        &edits,
        &preconditions,
    ) else {
        warn!("survival break rollback lost its exact world precondition");
        return Err(SimulationRequestError::WorldMutationFailed);
    };
    if rollback.applied.len() != edits.len() {
        warn!(
            expected = edits.len(),
            applied = rollback.applied.len(),
            "survival break rollback produced a partial world edit"
        );
        return Err(SimulationRequestError::WorldMutationFailed);
    }
    Ok(())
}

impl SessionRegistry {
    pub(in crate::play) fn commit_survival_break(
        &self,
        authority: &SimulationAuthority,
        storage: &mut mc_world::WorldStorage,
        block_light: Option<&BlockLightTable>,
        actor_session: SessionId,
        plan: &SurvivalBreakPlan,
    ) -> Result<Option<CommittedSurvivalBreak>, SimulationRequestError> {
        let player_state = {
            let inner = self.lock_inner("prepare survival break");
            let Some(player_state) = inner.player_persistence.get(&actor_session).cloned() else {
                return Ok(None);
            };
            player_state
        };
        let tool_slot = PlayerInventory::HOTBAR_BASE + usize::from(plan.held.hotbar_slot);
        let (expected_inventory, updated_inventory, changed_slots) = {
            let wait_started = Instant::now();
            let guard = crate::lock_policy::lock_authoritative_mutex(
                &player_state,
                "play.player_persistence",
            );
            let player = crate::lock_metrics::timed_guard(
                crate::lock_metrics::LockMetricKind::PlayerPersistence,
                "snapshot survival break inventory",
                wait_started,
                guard,
            );
            if player.selected_hotbar_slot != plan.held.hotbar_slot
                || player.inventory.slots[tool_slot] != plan.held.expected
            {
                return Ok(None);
            }
            let expected_inventory = player.inventory.clone();
            let mut updated_inventory = expected_inventory.clone();
            let changed_slots = if let Some(max_damage) = plan.held.max_damage {
                if plan.held.expected.is_empty() {
                    return Err(SimulationRequestError::InvalidCommand);
                }
                let held = &mut updated_inventory.slots[tool_slot];
                let new_damage = held.damage.unwrap_or(0).saturating_add(1);
                if new_damage >= max_damage {
                    *held = ItemStack::EMPTY;
                } else {
                    held.damage = Some(new_damage);
                }
                vec![(tool_slot, held.clone())]
            } else {
                Vec::new()
            };
            (expected_inventory, updated_inventory, changed_slots)
        };

        let Some(block) = apply_block_edit_batch_to_storage_conditionally(
            storage,
            block_light,
            &plan.edits,
            &plan.preconditions,
        ) else {
            return Ok(None);
        };
        if block.applied.len() != plan.edits.len() {
            warn!(
                session_id = actor_session,
                expected = plan.edits.len(),
                applied = block.applied.len(),
                "survival break preflight produced a partial world edit"
            );
            return Err(SimulationRequestError::WorldMutationFailed);
        }

        let committed_inventory = updated_inventory.clone();
        let inventory_committed = {
            let inner = self.lock_inner("commit survival break session");
            let session_is_current = inner
                .player_persistence
                .get(&actor_session)
                .is_some_and(|current| Arc::ptr_eq(current, &player_state));
            if !session_is_current {
                false
            } else {
                let wait_started = Instant::now();
                let guard = crate::lock_policy::lock_authoritative_mutex(
                    &player_state,
                    "play.player_persistence",
                );
                let mut player = crate::lock_metrics::timed_guard(
                    crate::lock_metrics::LockMetricKind::PlayerPersistence,
                    "publish survival break inventory",
                    wait_started,
                    guard,
                );
                if player.selected_hotbar_slot != plan.held.hotbar_slot
                    || player.inventory.slots != expected_inventory.slots
                {
                    false
                } else {
                    player.replace_inventory(committed_inventory);
                    true
                }
            }
        };
        if !inventory_committed {
            rollback_survival_break(storage, block_light, &block)?;
            return Ok(None);
        }
        let inventory = updated_inventory;
        let dispatches = self.spawn_item_drops_owned(
            authority,
            plan.drops
                .iter()
                .map(|drop| (drop.entity_type_id, drop.position, drop.stack.clone())),
        );
        Ok(Some(CommittedSurvivalBreak {
            block,
            inventory,
            changed_slots,
            dispatches,
        }))
    }

    pub(in crate::play) fn commit_survival_placement(
        &self,
        _authority: &SimulationAuthority,
        storage: &mut mc_world::WorldStorage,
        block_light: Option<&BlockLightTable>,
        actor_session: SessionId,
        plan: &SurvivalPlacementPlan,
    ) -> Result<Option<CommittedSurvivalPlacement>, SimulationRequestError> {
        let inner = self.lock_inner("commit survival placement");
        let Some(player_state) = inner.player_persistence.get(&actor_session).cloned() else {
            return Ok(None);
        };
        let wait_started = Instant::now();
        let guard =
            crate::lock_policy::lock_authoritative_mutex(&player_state, "play.player_persistence");
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit survival placement",
            wait_started,
            guard,
        );
        let Some(consume_held_item) =
            placement_inventory_debit(player_state.game_mode, plan.expected_game_mode)
        else {
            return Ok(None);
        };
        let held_slot = plan.held.inventory_slot;
        if held_slot != PlayerInventory::OFFHAND_SLOT
            && held_slot
                != PlayerInventory::HOTBAR_BASE + usize::from(player_state.selected_hotbar_slot)
        {
            return Ok(None);
        }
        if player_state.inventory.slots[held_slot] != plan.held.expected {
            return Ok(None);
        }
        let mut inventory = player_state.inventory.clone();
        let changed_slots = if consume_held_item {
            let held = &mut inventory.slots[held_slot];
            held.count = held.count.saturating_sub(1);
            if held.count <= 0 {
                *held = ItemStack::EMPTY;
            }
            vec![(held_slot, held.clone())]
        } else {
            Vec::new()
        };

        let Some(block) = apply_block_edit_batch_with_scheduled_ticks_to_storage_conditionally(
            storage,
            block_light,
            &plan.edits,
            &plan.preconditions,
            &plan.scheduled_block_ticks,
        ) else {
            return Ok(None);
        };
        if block.applied.len() != plan.edits.len() {
            warn!(
                session_id = actor_session,
                expected = plan.edits.len(),
                applied = block.applied.len(),
                "survival placement preflight produced a partial world edit"
            );
            return Err(SimulationRequestError::WorldMutationFailed);
        }
        if consume_held_item {
            player_state.replace_inventory(inventory.clone());
        }
        Ok(Some(CommittedSurvivalPlacement {
            block,
            inventory,
            changed_slots,
        }))
    }

    pub(in crate::play) fn prepare_survival_placement_transaction(
        &self,
        actor_session: SessionId,
    ) -> Option<SurvivalPlacementTransaction> {
        let inner = self.lock_inner("prepare regional survival placement");
        inner
            .player_persistence
            .get(&actor_session)
            .cloned()
            .map(|player_state| SurvivalPlacementTransaction { player_state })
    }

    pub(in crate::play) fn prepare_survival_break_transaction(
        &self,
        actor_session: SessionId,
    ) -> Option<SurvivalBreakTransaction> {
        let inner = self.lock_inner("prepare regional survival break");
        inner
            .player_persistence
            .get(&actor_session)
            .cloned()
            .map(|player_state| SurvivalBreakTransaction { player_state })
    }

    pub(in crate::play) fn prepare_bucket_use_transaction(
        &self,
        actor_session: SessionId,
    ) -> Option<BucketUseTransaction> {
        let inner = self.lock_inner("prepare regional bucket use");
        if !inner.sessions.contains_key(&actor_session) {
            return None;
        }
        inner
            .player_persistence
            .get(&actor_session)
            .cloned()
            .map(|player_state| BucketUseTransaction { player_state })
    }

    pub(in crate::play) fn commit_bucket_use(
        &self,
        _authority: &SimulationAuthority,
        storage: &mut mc_world::WorldStorage,
        block_light: Option<&BlockLightTable>,
        actor_session: SessionId,
        plan: &BucketUsePlan,
    ) -> Result<Option<CommittedBucketUse>, SimulationRequestError> {
        let inner = self.lock_inner("commit bucket use");
        if !inner.sessions.contains_key(&actor_session) {
            return Ok(None);
        }
        let Some(player_state) = inner.player_persistence.get(&actor_session).cloned() else {
            return Ok(None);
        };
        let wait_started = Instant::now();
        let guard =
            crate::lock_policy::lock_authoritative_mutex(&player_state, "play.player_persistence");
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit bucket use",
            wait_started,
            guard,
        );
        let (inventory, changed_slots) = if let Some(change) = &plan.inventory {
            if player_state.inventory.slots[change.held_slot] != change.expected_held {
                return Ok(None);
            }
            let Some((inventory, changed_slots)) = plan_bucket_replacement(
                &player_state.inventory,
                change.held_slot,
                change.replacement_item,
                change.replacement_max_stack,
            ) else {
                return Ok(None);
            };
            (Some(inventory), changed_slots)
        } else {
            (None, Vec::new())
        };

        let Some(block) = apply_block_edit_batch_to_storage_conditionally(
            storage,
            block_light,
            std::slice::from_ref(&plan.edit),
            std::slice::from_ref(&plan.precondition),
        ) else {
            return Ok(None);
        };
        if block.applied.len() != 1 {
            return Err(SimulationRequestError::WorldMutationFailed);
        }
        if let Some(inventory) = &inventory {
            player_state.replace_inventory(inventory.clone());
        }
        Ok(Some(CommittedBucketUse {
            block,
            inventory,
            changed_slots,
        }))
    }
}
