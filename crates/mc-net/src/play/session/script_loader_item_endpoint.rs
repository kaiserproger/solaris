use std::sync::Arc;

use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_protocol::packets::play::ItemStack;
use mc_script::ScriptPlayerInventoryFailure;
use tokio::sync::oneshot;

use crate::play::inventory::{PlayerInventory, item_max_stack};
use crate::play::persistence::PlayerPersistedState;

use super::SessionRegistry;
use super::outbound::{OutboundCommand, dispatch_visibility_command};
use super::script_inventory_transaction_endpoint::ScriptPlayerInventoryReservation;
use super::visibility::ordered_session_recipient;

#[derive(Debug)]
pub(in crate::play) struct LoaderItemGrantCommand {
    stack: ItemStack,
    completion: Option<oneshot::Sender<Result<(), ScriptPlayerInventoryFailure>>>,
    reservation: ScriptPlayerInventoryReservation,
}

impl LoaderItemGrantCommand {
    pub(in crate::play) fn stack(&self) -> &ItemStack {
        &self.stack
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
    pub(crate) async fn route_loader_item_grant(
        &self,
        player_id: u64,
        block_id: &str,
        stack: ItemStack,
    ) -> Result<(), ScriptPlayerInventoryFailure> {
        let transaction_gate = {
            let inner = self.lock_inner("route Loader item grant");
            let Some(session) = inner.sessions.get(&player_id) else {
                return Err(ScriptPlayerInventoryFailure::PlayerUnavailable);
            };
            if session.tx.is_closed()
                || session
                    .loader_session
                    .as_ref()
                    .and_then(|loader| loader.block_state_id(block_id))
                    .is_none()
            {
                return Err(ScriptPlayerInventoryFailure::PlayerUnavailable);
            }
            Arc::clone(&session.script_inventory_transaction_gate)
        };
        let Some((reservation, recipient)) = transaction_gate.reserve_owner(|| {
            let inner = self.lock_inner("reserve Loader item grant owner");
            let session = inner.sessions.get(&player_id)?;
            if session.tx.is_closed()
                || !Arc::ptr_eq(
                    &session.script_inventory_transaction_gate,
                    &transaction_gate,
                )
                || session
                    .loader_session
                    .as_ref()
                    .and_then(|loader| loader.block_state_id(block_id))
                    .is_none()
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
            OutboundCommand::LoaderItemGrant(LoaderItemGrantCommand {
                stack,
                completion: Some(completion),
                reservation,
            }),
        );
        result
            .await
            .unwrap_or(Err(ScriptPlayerInventoryFailure::PlayerUnavailable))
    }
}

pub(in crate::play) fn apply_loader_item_grant(
    stack: &ItemStack,
    inventory: &mut PlayerInventory,
    persisted: &mut PlayerPersistedState,
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
) -> Result<(), ScriptPlayerInventoryFailure> {
    let mut updated = inventory.clone();
    let max_stack = item_max_stack(item_facts, items, stack);
    let (remaining, _) = updated.merge_stack(stack.clone(), max_stack);
    if !remaining.is_empty() {
        return Err(ScriptPlayerInventoryFailure::InventoryFull);
    }
    *inventory = updated.clone();
    persisted.replace_inventory(updated);
    Ok(())
}
