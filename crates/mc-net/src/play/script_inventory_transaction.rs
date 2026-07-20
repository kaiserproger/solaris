use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_protocol::codec::Identifier;
use mc_protocol::packets::play::ItemStack;
use mc_script::{ScriptInventoryStorageTransaction, ScriptStorageMutation};

use super::inventory::{PlayerInventory, item_max_stack};

/// Prepared storage side supplied by the storage actor. The adapter owns
/// inventory planning; storage owns CAS, quotas, revision allocation, and its
/// durable journal. The future storage implementation must recheck and commit
/// this value in the same simulation-owner turn as the inventory CAS.
pub(crate) trait ScriptStorageTransactionPrepare {
    type Prepared;
    type Error;

    fn prepare(
        &mut self,
        plugin_id: &str,
        mutations: &[ScriptStorageMutation],
    ) -> Result<ScriptStoragePrepareOutcome<Self::Prepared>, Self::Error>;

    fn commit(&mut self, prepared: Self::Prepared) -> Result<(), Self::Error>;
}

pub(crate) enum ScriptStoragePrepareOutcome<T> {
    Prepared(T),
    Rejected,
}

#[derive(Debug, Clone)]
pub(crate) struct ScriptInventoryPlan {
    pub(crate) expected: PlayerInventory,
    pub(crate) updated: PlayerInventory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScriptInventoryPlanError {
    UnknownResource(String),
    InsufficientResource(String),
    InventoryFull(String),
}

/// Plan every player-inventory mutation before storage is touched. Script
/// transactions are deliberately restricted to main inventory and hotbar
/// slots, 9 through 44; armor, offhand, crafting, and cursor state cannot be
/// used to satisfy a plugin request.
pub(crate) fn plan_script_inventory_transaction(
    transaction: &ScriptInventoryStorageTransaction,
    inventory: &PlayerInventory,
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
) -> Result<ScriptInventoryPlan, ScriptInventoryPlanError> {
    let mut updated = inventory.clone();

    for delta in transaction
        .inventory()
        .iter()
        .filter(|delta| delta.delta() < 0)
    {
        let item_id = resolve_item_id(items, delta.resource_id())?;
        let mut remaining = i32::from(delta.delta().unsigned_abs());
        for slot in 9..=44 {
            let stack = &mut updated.slots[slot];
            if stack.item_id != item_id || stack.is_empty() {
                continue;
            }
            let removed = stack.count.min(remaining);
            stack.count -= removed;
            remaining -= removed;
            if stack.count == 0 {
                *stack = ItemStack::EMPTY;
            }
            if remaining == 0 {
                break;
            }
        }
        if remaining != 0 {
            return Err(ScriptInventoryPlanError::InsufficientResource(
                delta.resource_id().to_owned(),
            ));
        }
    }

    for delta in transaction
        .inventory()
        .iter()
        .filter(|delta| delta.delta() > 0)
    {
        let item_id = resolve_item_id(items, delta.resource_id())?;
        let stack = ItemStack::new(item_id, i32::from(delta.delta()));
        let max_stack = item_max_stack(item_facts, items, &stack);
        let (remaining, _) = updated.merge_stack(stack, max_stack);
        if !remaining.is_empty() {
            return Err(ScriptInventoryPlanError::InventoryFull(
                delta.resource_id().to_owned(),
            ));
        }
    }

    Ok(ScriptInventoryPlan {
        expected: inventory.clone(),
        updated,
    })
}

fn resolve_item_id(items: &ItemRegistry, resource: &str) -> Result<u32, ScriptInventoryPlanError> {
    let identifier = Identifier::parse(resource.to_owned())
        .map_err(|_| ScriptInventoryPlanError::UnknownResource(resource.to_owned()))?;
    items
        .id_of(&identifier)
        .ok_or_else(|| ScriptInventoryPlanError::UnknownResource(resource.to_owned()))
}
