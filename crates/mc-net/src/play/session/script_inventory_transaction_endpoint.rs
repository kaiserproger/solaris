use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Instant;

use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_script::ScriptInventoryStorageTransaction;

use crate::lock_policy::{lock_authoritative_mutex, resolve_authoritative_lock};
use crate::play::script_inventory_transaction::{
    ScriptStoragePrepareOutcome, ScriptStorageTransactionPrepare, plan_script_inventory_transaction,
};

use super::SessionRegistry;
use super::outbound::{OutboundCommand, dispatch_visibility_command};
use super::visibility::ordered_session_recipient;

#[derive(Debug)]
pub(super) struct ScriptInventoryTransactionGate {
    state: Mutex<ScriptInventoryTransactionGateState>,
    changed: Condvar,
}

#[derive(Debug)]
struct ScriptInventoryTransactionGateState {
    active: bool,
    pending_owner_transactions: usize,
}

#[derive(Debug)]
pub(super) struct ScriptPlayerInventoryReservation {
    gate: Arc<ScriptInventoryTransactionGate>,
}

#[derive(Debug)]
pub(in crate::play) struct ScriptInventoryTransactionGuard<'a> {
    _state: MutexGuard<'a, ScriptInventoryTransactionGateState>,
}

impl ScriptInventoryTransactionGate {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(ScriptInventoryTransactionGateState {
                active: true,
                pending_owner_transactions: 0,
            }),
            changed: Condvar::new(),
        }
    }

    pub(super) fn reserve_owner<T>(
        self: &Arc<Self>,
        capture: impl FnOnce() -> Option<T>,
    ) -> Option<(ScriptPlayerInventoryReservation, T)> {
        let mut state = self.lock("reserve session-owner inventory transaction", None);
        if !state.active {
            return None;
        }
        let captured = capture()?;
        state.pending_owner_transactions += 1;
        Some((
            ScriptPlayerInventoryReservation {
                gate: Arc::clone(self),
            },
            captured,
        ))
    }

    fn begin_compound(&self, player_id: u64) -> Option<ScriptInventoryTransactionGuard<'_>> {
        let mut state = self.lock("begin compound inventory transaction", Some(player_id));
        while state.active && state.pending_owner_transactions != 0 {
            state = resolve_authoritative_lock(
                self.changed.wait(state),
                "script.inventory_transaction_gate",
            );
        }
        state
            .active
            .then_some(ScriptInventoryTransactionGuard { _state: state })
    }

    pub(super) fn close(&self, player_id: u64) {
        let mut state = self.lock("close inventory transaction gate", Some(player_id));
        state.active = false;
        self.changed.notify_all();
    }

    fn lock(
        &self,
        _operation: &'static str,
        _player_id: Option<u64>,
    ) -> MutexGuard<'_, ScriptInventoryTransactionGateState> {
        lock_authoritative_mutex(&self.state, "script.inventory_transaction_gate")
    }
}

impl ScriptPlayerInventoryReservation {
    pub(in crate::play) fn begin_commit(&self) -> Option<ScriptInventoryTransactionGuard<'_>> {
        let state = self.gate.lock("begin session-owner inventory commit", None);
        state
            .active
            .then_some(ScriptInventoryTransactionGuard { _state: state })
    }
}

impl Drop for ScriptPlayerInventoryReservation {
    fn drop(&mut self) {
        let mut state = self
            .gate
            .lock("finish session-owner inventory transaction", None);
        state.pending_owner_transactions = state
            .pending_owner_transactions
            .checked_sub(1)
            .expect("owner inventory reservation count remains balanced");
        self.gate.changed.notify_all();
    }
}

impl SessionRegistry {
    /// Commits the plugin ledger and canonical player inventory behind the same
    /// session gate used by standalone owner-routed inventory transactions.
    /// Storage I/O stays inside the gate because releasing it would make the
    /// ledger and inventory commits observable apart.
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
        let transaction_gate = {
            let inner = self.lock_inner("prepare script inventory transaction");
            let player_id = transaction.player_id().value();
            let Some(session) = inner.sessions.get(&player_id) else {
                return Ok(false);
            };
            if session.tx.is_closed() {
                return Ok(false);
            }
            Arc::clone(&session.script_inventory_transaction_gate)
        };

        #[cfg(test)]
        self.pause_script_transaction_after_capture_for_test();

        let player_id = transaction.player_id().value();
        let Some(_transaction_guard) = transaction_gate.begin_compound(player_id) else {
            return Ok(false);
        };
        let (player_state, recipient) = {
            let inner = self.lock_inner("capture compound inventory transaction owner");
            let Some(session) = inner.sessions.get(&player_id) else {
                return Ok(false);
            };
            if session.tx.is_closed()
                || !Arc::ptr_eq(
                    &session.script_inventory_transaction_gate,
                    &transaction_gate,
                )
            {
                return Ok(false);
            }
            let Some(player_state) = inner.player_persistence.get(&player_id).cloned() else {
                return Ok(false);
            };
            (player_state, ordered_session_recipient(player_id, session))
        };

        let wait_started = Instant::now();
        let guard = lock_authoritative_mutex(&player_state, "play.player_persistence");
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
    pub(super) fn pause_script_transaction_after_capture_for_test(&self) {
        let probe = self
            .script_transaction_capture_probe
            .lock()
            .expect("test lock poisoned")
            .take();
        if let Some(probe) = probe {
            probe.reached.send(()).expect("capture probe receiver");
            probe.resume.recv().expect("capture probe resume");
        }
    }
}
