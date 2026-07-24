use std::sync::Arc;

use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_script::{ScriptPlayerInventoryFailure, ScriptPlayerInventoryTransaction};
use tokio::sync::oneshot;

use crate::play::inventory::PlayerInventory;
use crate::play::persistence::PlayerPersistedState;
use crate::play::script_inventory_transaction::{
    ScriptInventoryPlanError, plan_script_inventory_deltas,
};

use super::SessionRegistry;
use super::outbound::{OutboundCommand, dispatch_visibility_command};
use super::script_inventory_transaction_endpoint::ScriptPlayerInventoryReservation;
use super::visibility::ordered_session_recipient;

#[derive(Debug)]
pub(in crate::play) struct ScriptPlayerInventoryCommand {
    transaction: ScriptPlayerInventoryTransaction,
    completion: Option<oneshot::Sender<Result<(), ScriptPlayerInventoryFailure>>>,
    reservation: ScriptPlayerInventoryReservation,
}

impl ScriptPlayerInventoryCommand {
    pub(in crate::play) fn transaction(&self) -> &ScriptPlayerInventoryTransaction {
        &self.transaction
    }

    pub(in crate::play) fn begin_commit(
        &self,
    ) -> Option<super::script_inventory_transaction_endpoint::ScriptInventoryTransactionGuard<'_>>
    {
        self.reservation.begin_commit()
    }

    pub(in crate::play) fn complete(mut self, result: Result<(), ScriptPlayerInventoryFailure>) {
        if let Some(completion) = self.completion.take() {
            let _ = completion.send(result);
        }
    }
}

impl SessionRegistry {
    /// Push one inventory transaction to the exact session owner and await its
    /// authoritative commit result.
    pub(crate) async fn route_script_player_inventory_transaction(
        &self,
        transaction: ScriptPlayerInventoryTransaction,
    ) -> Result<(), ScriptPlayerInventoryFailure> {
        let transaction_gate = {
            let inner = self.lock_inner("route script player inventory transaction");
            let player_id = transaction.player_id().value();
            let Some(session) = inner.sessions.get(&player_id) else {
                return Err(ScriptPlayerInventoryFailure::PlayerUnavailable);
            };
            if session.tx.is_closed() {
                return Err(ScriptPlayerInventoryFailure::PlayerUnavailable);
            }
            Arc::clone(&session.script_inventory_transaction_gate)
        };
        let player_id = transaction.player_id().value();
        let Some((reservation, recipient)) = transaction_gate.reserve_owner(|| {
            let inner = self.lock_inner("reserve script player inventory owner");
            let session = inner.sessions.get(&player_id)?;
            if session.tx.is_closed()
                || !Arc::ptr_eq(
                    &session.script_inventory_transaction_gate,
                    &transaction_gate,
                )
            {
                return None;
            }
            Some(ordered_session_recipient(player_id, session))
        }) else {
            return Err(ScriptPlayerInventoryFailure::PlayerUnavailable);
        };
        let (completion, result) = oneshot::channel();
        dispatch_visibility_command(
            &recipient,
            OutboundCommand::ScriptPlayerInventoryTransaction(ScriptPlayerInventoryCommand {
                transaction,
                completion: Some(completion),
                reservation,
            }),
        );
        result
            .await
            .unwrap_or(Err(ScriptPlayerInventoryFailure::PlayerUnavailable))
    }
}

pub(in crate::play) fn apply_script_player_inventory_transaction(
    transaction: &ScriptPlayerInventoryTransaction,
    inventory: &mut PlayerInventory,
    persisted: &mut PlayerPersistedState,
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
) -> Result<(), ScriptPlayerInventoryFailure> {
    let plan = plan_script_inventory_deltas(transaction.deltas(), inventory, items, item_facts)
        .map_err(map_plan_failure)?;
    *inventory = plan.updated.clone();
    persisted.replace_inventory(plan.updated);
    Ok(())
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
