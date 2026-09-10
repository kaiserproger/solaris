use std::collections::BTreeMap;

use mc_data::ItemStack;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_script::{
    ScriptInventoryEnchantment, ScriptInventoryEndpoint, ScriptInventoryFence, ScriptInventoryItem,
    ScriptInventorySlot, ScriptOperationFailure, ScriptOwnedInventorySnapshot,
    ScriptOwnedItemTransfer,
};
use sha2::{Digest, Sha256};

use super::inventory::{can_stack, item_max_stack};

// Transitional module: exercised by owned_inventory tests until the C1 runtime
// wires canonical POI/resident endpoints; production callers land with it.
pub(crate) fn owned_inventory_snapshot(
    endpoint: ScriptInventoryEndpoint,
    revision: u64,
    inventory: &[ItemStack],
    items: &ItemRegistry,
) -> Result<ScriptOwnedInventorySnapshot, ScriptOperationFailure> {
    let (start, slots) = match &endpoint {
        ScriptInventoryEndpoint::PlayerInventory { .. } => (
            9,
            inventory
                .get(9..45)
                .ok_or(ScriptOperationFailure::InvalidRequest)?,
        ),
        ScriptInventoryEndpoint::Warehouse { .. }
            if inventory.len() <= mc_script::MAX_OWNED_INVENTORY_SLOTS =>
        {
            (0, inventory)
        }
        _ => return Err(ScriptOperationFailure::InvalidRequest),
    };
    let slots = slots
        .iter()
        .enumerate()
        .map(|(index, stack)| {
            let item = if stack.is_empty() {
                None
            } else {
                let name = items
                    .name_of(stack.item_id)
                    .ok_or(ScriptOperationFailure::InvalidRequest)?;
                Some(ScriptInventoryItem::new(
                    name.as_str().to_owned(),
                    stack.count as u32,
                    stack.damage,
                    stack
                        .enchantments
                        .iter()
                        .map(|enchantment| {
                            ScriptInventoryEnchantment::new(
                                enchantment.id.as_str().to_owned(),
                                enchantment.level,
                            )
                        })
                        .collect(),
                    stack.custom_name.clone(),
                    stack
                        .item_model
                        .as_ref()
                        .map(|model| model.as_str().to_owned()),
                ))
            };
            Ok(ScriptInventorySlot::new((start + index) as u8, item))
        })
        .collect::<Result<Vec<_>, ScriptOperationFailure>>()?;
    let mut hash = Sha256::new();
    serde_json::to_writer(&mut hash, &(&endpoint, &slots))
        .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
    let fence = ScriptInventoryFence::try_new(revision, format!("{:x}", hash.finalize()))
        .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
    Ok(ScriptOwnedInventorySnapshot::new(endpoint, fence, slots))
}

#[cfg(test)]
pub(crate) fn plan_owned_item_transfers(
    transfers: &[ScriptOwnedItemTransfer],
    inventories: &BTreeMap<ScriptInventoryEndpoint, Vec<ItemStack>>,
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
) -> Result<BTreeMap<ScriptInventoryEndpoint, Vec<ItemStack>>, ScriptOperationFailure> {
    if transfers.is_empty() || transfers.len() > mc_script::MAX_OWNED_INVENTORY_TRANSFERS {
        return Err(ScriptOperationFailure::InvalidRequest);
    }
    let mut updated = inventories.clone();
    for transfer in transfers {
        let count = i32::try_from(transfer.count)
            .ok()
            .filter(|count| *count > 0)
            .ok_or(ScriptOperationFailure::InvalidRequest)?;
        if transfer.source == transfer.destination
            && transfer.source_slot == transfer.destination_slot
        {
            return Err(ScriptOperationFailure::InvalidRequest);
        }
        let source = slot(&updated, &transfer.source, transfer.source_slot)?;
        if source.is_empty() || source.count < count {
            return Err(ScriptOperationFailure::InsufficientItems);
        }
        let destination = slot(&updated, &transfer.destination, transfer.destination_slot)?;
        let max_stack = item_max_stack(item_facts, items, source);
        let destination_count = if destination.is_empty() {
            0
        } else if can_stack(source, destination) {
            destination.count
        } else {
            return Err(ScriptOperationFailure::Blocked);
        };
        let total = destination_count
            .checked_add(count)
            .filter(|total| *total <= max_stack)
            .ok_or(ScriptOperationFailure::Capacity)?;
        let mut moved = source.clone();
        moved.count = total;
        let source = &mut updated.get_mut(&transfer.source).expect("validated source")
            [usize::from(transfer.source_slot)];
        source.count -= count;
        if source.count == 0 {
            *source = ItemStack::EMPTY;
        }
        updated
            .get_mut(&transfer.destination)
            .expect("validated destination")[usize::from(transfer.destination_slot)] = moved;
    }
    Ok(updated)
}

#[cfg(test)]
fn slot<'a>(
    inventories: &'a BTreeMap<ScriptInventoryEndpoint, Vec<ItemStack>>,
    endpoint: &ScriptInventoryEndpoint,
    index: u8,
) -> Result<&'a ItemStack, ScriptOperationFailure> {
    let allowed = match endpoint {
        ScriptInventoryEndpoint::PlayerInventory { .. } => (9..=44).contains(&index),
        ScriptInventoryEndpoint::Warehouse { .. } => {
            usize::from(index) < mc_script::MAX_OWNED_INVENTORY_SLOTS
        }
        _ => false,
    };
    if !allowed {
        return Err(ScriptOperationFailure::InvalidRequest);
    }
    inventories
        .get(endpoint)
        .ok_or(ScriptOperationFailure::NotFound)?
        .get(usize::from(index))
        .ok_or(ScriptOperationFailure::InvalidRequest)
}
