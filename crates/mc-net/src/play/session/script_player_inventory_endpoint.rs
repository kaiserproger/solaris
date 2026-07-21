use std::sync::Arc;
use std::time::Instant;

use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_script::{ScriptPlayerInventoryFailure, ScriptPlayerInventoryTransaction};
use tracing::warn;

use crate::play::script_inventory_transaction::{
    ScriptInventoryPlanError, plan_script_inventory_deltas,
};

use super::SessionRegistry;
use super::outbound::{OutboundCommand, dispatch_visibility_command};
use super::visibility::ordered_session_recipient;

impl SessionRegistry {
    pub(crate) fn commit_script_player_inventory_transaction(
        &self,
        transaction: &ScriptPlayerInventoryTransaction,
        items: &ItemRegistry,
        item_facts: &ItemFactsTable,
    ) -> Result<(), ScriptPlayerInventoryFailure> {
        let (player_state, recipient, transaction_active) = {
            let inner = self.lock_inner("prepare script player inventory transaction");
            let player_id = transaction.player_id().value();
            let Some(session) = inner.sessions.get(&player_id) else {
                return Err(ScriptPlayerInventoryFailure::PlayerUnavailable);
            };
            if session.tx.is_closed() {
                return Err(ScriptPlayerInventoryFailure::PlayerUnavailable);
            }
            let Some(player_state) = inner.player_persistence.get(&player_id).cloned() else {
                return Err(ScriptPlayerInventoryFailure::PlayerUnavailable);
            };
            (
                player_state,
                ordered_session_recipient(player_id, session),
                Arc::clone(&session.script_transaction_active),
            )
        };

        #[cfg(test)]
        self.pause_script_transaction_after_capture_for_test();

        let transaction_active = transaction_active.lock().unwrap_or_else(|poisoned| {
            warn!(
                player_id = transaction.player_id().value(),
                "script transaction gate was poisoned during player inventory commit; recovering state"
            );
            poisoned.into_inner()
        });
        if !*transaction_active {
            return Err(ScriptPlayerInventoryFailure::PlayerUnavailable);
        }

        let wait_started = Instant::now();
        let guard = player_state.lock().unwrap_or_else(|poisoned| {
            warn!(
                player_id = transaction.player_id().value(),
                "player persistence mutex was poisoned during script inventory commit; recovering state"
            );
            poisoned.into_inner()
        });
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit script player inventory transaction",
            wait_started,
            guard,
        );
        let plan = plan_script_inventory_deltas(
            transaction.deltas(),
            &player_state.inventory,
            items,
            item_facts,
        )
        .map_err(map_plan_failure)?;
        player_state.replace_inventory(plan.updated.clone());
        let carried_item = player_state.carried_item.clone();
        drop(player_state);

        dispatch_visibility_command(
            &recipient,
            OutboundCommand::AuthoritativeInventory {
                inventory: Box::new(plan.updated),
                carried_item,
            },
        );
        Ok(())
    }
}

fn map_plan_failure(error: ScriptInventoryPlanError) -> ScriptPlayerInventoryFailure {
    match error {
        ScriptInventoryPlanError::UnknownResource(_) => {
            ScriptPlayerInventoryFailure::UnknownResource
        }
        ScriptInventoryPlanError::InsufficientResource(_) => {
            ScriptPlayerInventoryFailure::InsufficientResource
        }
        ScriptInventoryPlanError::InventoryFull(_) => ScriptPlayerInventoryFailure::InventoryFull,
    }
}
