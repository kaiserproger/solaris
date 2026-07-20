use std::sync::Arc;
use std::time::Instant;

use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_script::ScriptInventoryStorageTransaction;
use tracing::warn;

use crate::play::script_inventory_transaction::{
    ScriptStoragePrepareOutcome, ScriptStorageTransactionPrepare, plan_script_inventory_transaction,
};

use super::SessionRegistry;
use super::outbound::{OutboundCommand, dispatch_visibility_command};
use super::visibility::ordered_session_recipient;

impl SessionRegistry {
    /// Commits the plugin ledger and the canonical player inventory while the
    /// same player-state lock excludes every other inventory mutation. Storage
    /// I/O is intentionally allowed under this lock: plugin purchases are a
    /// cold path, and releasing it would make the two commits observable apart.
    pub(crate) fn commit_script_inventory_storage_transaction<S>(
        &self,
        plugin_id: &str,
        transaction: &ScriptInventoryStorageTransaction,
        items: &ItemRegistry,
        item_facts: &ItemFactsTable,
        storage: &mut S,
    ) -> Result<bool, S::Error>
    where
        S: ScriptStorageTransactionPrepare,
    {
        let (player_state, recipient, transaction_active) = {
            let inner = self.lock_inner("prepare script inventory transaction");
            let player_id = transaction.player_id().value();
            let Some(session) = inner.sessions.get(&player_id) else {
                return Ok(false);
            };
            if session.tx.is_closed() {
                return Ok(false);
            }
            let Some(player_state) = inner.player_persistence.get(&player_id).cloned() else {
                return Ok(false);
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
                "script transaction gate was poisoned during commit; recovering state"
            );
            poisoned.into_inner()
        });
        if !*transaction_active {
            return Ok(false);
        }

        let wait_started = Instant::now();
        let guard = player_state.lock().unwrap_or_else(|poisoned| {
            warn!(
                player_id = transaction.player_id().value(),
                "player persistence mutex was poisoned during script inventory transaction; recovering state"
            );
            poisoned.into_inner()
        });
        let mut player_state = crate::lock_metrics::timed_guard(
            crate::lock_metrics::LockMetricKind::PlayerPersistence,
            "commit script inventory transaction",
            wait_started,
            guard,
        );
        let plan = match plan_script_inventory_transaction(
            transaction,
            &player_state.inventory,
            items,
            item_facts,
        ) {
            Ok(plan) => plan,
            Err(_) => return Ok(false),
        };
        let prepared = match storage.prepare(plugin_id, transaction.storage())? {
            ScriptStoragePrepareOutcome::Prepared(prepared) => prepared,
            ScriptStoragePrepareOutcome::Rejected => return Ok(false),
        };
        storage.commit(prepared)?;
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
        Ok(true)
    }

    #[cfg(test)]
    fn pause_script_transaction_after_capture_for_test(&self) {
        let probe = self
            .script_transaction_capture_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe.reached.send(()).expect("capture probe receiver");
            probe.resume.recv().expect("capture probe resume");
        }
    }
}
