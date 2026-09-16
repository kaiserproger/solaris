use std::collections::BTreeMap;
use std::sync::Arc;

use mc_data::ItemStack;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_script::{
    ScriptInventoryEnchantment, ScriptInventoryEndpoint, ScriptInventoryFence, ScriptInventoryItem,
    ScriptInventoryResourcePlan, ScriptInventorySlot, ScriptOperation, ScriptOperationFailure,
    ScriptOperationOutcome, ScriptOperationPayload, ScriptOperationRequest,
    ScriptOwnedInventorySnapshot, ScriptOwnedItemTransfer,
};
use mc_world::FurnaceSlot;
use sha2::{Digest, Sha256};

use super::inventory::{can_stack, item_max_stack};
use super::persistence::inventory_recovery::PlayerInventoryRecovery;
use super::script_inventory_transaction::{ScriptStorageCommitError, ScriptStoragePrepareOutcome};

/// One durable owned-inventory decision. The implementing adapter appends the
/// plugin storage operation receipt and, when a player endpoint participates,
/// the canonical player inventory after-image to the same recoverable world
/// journal decision. Both are replayed together at startup, so a crash can
/// never leave items inside the plugin ledger without the matching inventory.
pub(crate) trait OwnedInventoryPrepare {
    type Prepared;
    type Error: From<std::io::Error>;

    fn prepare_owned(
        &mut self,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        payload: ScriptOperationPayload,
        inventory: Option<PlayerInventoryRecovery>,
    ) -> Result<ScriptStoragePrepareOutcome<Self::Prepared>, Self::Error>;

    /// Commit the prepared decision and return its persisted world watermark.
    fn commit_owned(
        &mut self,
        prepared: Self::Prepared,
    ) -> Result<u64, ScriptStorageCommitError<Self::Error>>;

    /// Canonical durable gear of one resident endpoint, or the typed refusal
    /// that keeps a transfer from guessing at a foreign or absent resident.
    fn resident_endpoint_state(
        &self,
        plugin_id: &str,
        endpoint: &ScriptInventoryEndpoint,
    ) -> Result<ResidentEndpointState, ScriptOperationFailure>;

    /// Prepare the combined resident-gear decision: the C1 receipt, the changed
    /// resident gear records and the player inventory after-image in one frame.
    fn prepare_resident_gear(
        &mut self,
        plugin_id: &str,
        request: &ScriptOperationRequest,
        payload: ScriptOperationPayload,
        updates: Vec<ResidentGearUpdate>,
        inventory: Option<PlayerInventoryRecovery>,
    ) -> Result<ScriptStoragePrepareOutcome<Self::Prepared>, Self::Error>;

    /// The transaction id the next prepared gear batch will commit with; the
    /// fence revision a resident endpoint reads back from this transfer.
    fn gear_revision(&self) -> u64;
}

/// One durable resident gear stack as it crosses the storage boundary: the
/// canonical item identity and every component a transfer must preserve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResidentGearStack {
    pub(crate) resource_id: String,
    pub(crate) count: u32,
    pub(crate) damage: Option<i32>,
    pub(crate) enchantments: Vec<(String, i32)>,
    pub(crate) custom_name: Option<String>,
    pub(crate) item_model: Option<String>,
}

/// Canonical gear and revision of one resident endpoint.
#[derive(Debug, Clone)]
pub(crate) struct ResidentEndpointState {
    pub(crate) entity_uuid: String,
    pub(crate) revision: u64,
    pub(crate) equipment: Vec<Option<ResidentGearStack>>,
    pub(crate) carry: Vec<Option<ResidentGearStack>>,
}

/// The resident gear one transfer leaves behind, keyed by resident handle.
#[derive(Debug, Clone)]
pub(crate) struct ResidentGearUpdate {
    pub(crate) handle: String,
    pub(crate) entity_uuid: String,
    pub(crate) equipment: Vec<Option<ResidentGearStack>>,
    pub(crate) carry: Vec<Option<ResidentGearStack>>,
}

/// Project one durable resident stack onto a canonical engine item, preserving
/// durability, enchantments and the named/model components.
pub(crate) fn resident_gear_to_item(
    gear: &ResidentGearStack,
    items: &ItemRegistry,
) -> Result<ItemStack, ScriptOperationFailure> {
    let identifier = mc_protocol::codec::Identifier::parse(gear.resource_id.clone())
        .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
    let item_id = items
        .id_of(&identifier)
        .ok_or(ScriptOperationFailure::InvalidRequest)?;
    let mut stack = ItemStack::new(
        item_id,
        i32::try_from(gear.count).map_err(|_| ScriptOperationFailure::InvalidRequest)?,
    );
    stack.damage = gear.damage;
    stack.enchantments = gear
        .enchantments
        .iter()
        .map(|(id, level)| {
            Ok(mc_data::ItemEnchantment {
                id: mc_protocol::codec::Identifier::parse(id.clone())
                    .map_err(|_| ScriptOperationFailure::InvalidRequest)?,
                level: *level,
            })
        })
        .collect::<Result<Vec<_>, ScriptOperationFailure>>()?;
    stack.custom_name = gear.custom_name.clone();
    stack.item_model = match &gear.item_model {
        Some(model) => Some(Arc::new(
            mc_protocol::codec::Identifier::parse(model.clone())
                .map_err(|_| ScriptOperationFailure::InvalidRequest)?,
        )),
        None => None,
    };
    Ok(stack)
}

/// Project one canonical engine item onto a durable resident stack; an empty
/// stack is `None`, so the caller sees exactly the occupied slots.
pub(crate) fn item_to_resident_gear(
    stack: &ItemStack,
    items: &ItemRegistry,
) -> Result<ResidentGearStack, ScriptOperationFailure> {
    let name = items
        .name_of(stack.item_id)
        .ok_or(ScriptOperationFailure::InvalidRequest)?;
    Ok(ResidentGearStack {
        resource_id: name.as_str().to_owned(),
        count: u32::try_from(stack.count)
            .ok()
            .filter(|count| *count > 0)
            .ok_or(ScriptOperationFailure::InvalidRequest)?,
        damage: stack.damage,
        enchantments: stack
            .enchantments
            .iter()
            .map(|enchantment| (enchantment.id.as_str().to_owned(), enchantment.level))
            .collect(),
        custom_name: stack.custom_name.clone(),
        item_model: stack
            .item_model
            .as_ref()
            .map(|model| model.as_str().to_owned()),
    })
}

/// One canonical resident endpoint vector, with empty slots as `ItemStack::EMPTY`.
pub(crate) fn gear_slots_to_items(
    slots: &[Option<ResidentGearStack>],
    items: &ItemRegistry,
) -> Result<Vec<ItemStack>, ScriptOperationFailure> {
    slots
        .iter()
        .map(|slot| match slot {
            Some(gear) => resident_gear_to_item(gear, items),
            None => Ok(ItemStack::EMPTY),
        })
        .collect()
}

/// One planned canonical endpoint vector back onto durable resident slots.
pub(crate) fn items_to_gear_slots(
    slots: &[ItemStack],
    items: &ItemRegistry,
) -> Result<Vec<Option<ResidentGearStack>>, ScriptOperationFailure> {
    slots
        .iter()
        .map(|stack| {
            if stack.is_empty() {
                Ok(None)
            } else {
                item_to_resident_gear(stack, items).map(Some)
            }
        })
        .collect()
}

/// Canonical engine item of one runtime container slot.
///
/// The live settlement adapter reads chest/barrel block entities through this
/// projection, so a warehouse snapshot names items exactly like every other
/// owned-inventory endpoint and never invents a second item representation.
pub(crate) fn container_slot_to_item(slot: &FurnaceSlot) -> ItemStack {
    super::containers::furnace_slot_to_stack(slot)
}

/// The container block-entity image of one 27-slot canonical projection.
///
/// The inverse of [`container_slot_to_item`], and the image the server-owned
/// warehouse composite fences the container with. The menu planner keeps its
/// own item-to-slot projection private to the container module, which the
/// warehouse path cannot reach; the struct literal makes both compile against
/// the same slot field set, so a new field breaks this one too instead of
/// drifting silently.
pub(crate) fn container_chest_image(slots: &[ItemStack]) -> Option<mc_world::ChestBlockEntity> {
    let slots: [FurnaceSlot; 27] = slots
        .iter()
        .map(|stack| {
            if stack.is_empty() {
                FurnaceSlot::EMPTY
            } else {
                FurnaceSlot {
                    count: stack.count,
                    item_id: stack.item_id,
                    damage: stack.damage,
                    enchantments: stack.enchantments.clone(),
                    custom_name: stack.custom_name.clone(),
                    item_model: stack.item_model.clone(),
                    stew_effects: stack.stew_effects.clone(),
                }
            }
        })
        .collect::<Vec<_>>()
        .try_into()
        .ok()?;
    Some(mc_world::ChestBlockEntity { slots })
}

/// The player participant of one server-owned deposit.
///
/// Present when a real player's inventory moves with the container. Absent for
/// a deposit whose other participant is a plugin-owned record (a worker's
/// cargo): that record rides the encoded receipt instead, and no session is
/// fenced or after-imaged.
#[derive(Debug, Clone)]
pub(crate) struct WarehousePlayerParticipant {
    pub(crate) actor_id: u64,
    /// The actor's expected and planned canonical inventory.
    pub(crate) expected_inventory: Vec<ItemStack>,
    pub(crate) expected_carried_item: ItemStack,
    pub(crate) updated_inventory: Vec<ItemStack>,
    pub(crate) updated_carried_item: ItemStack,
}

/// One planned server-owned warehouse deposit, as its caller observed it.
///
/// The caller has already resolved the durable warehouse binding, read the
/// loaded container through the same projection a warehouse snapshot uses, and
/// planned the move against every other participant; this carries only what the
/// composite fences and commits, so the world half stays an adapter over the
/// ordered container command.
#[derive(Debug, Clone)]
pub(crate) struct WarehouseTransferRequest {
    pub(crate) position: mc_world::BlockPos,
    /// The container's canonical state id, as the caller observed it.
    pub(crate) expected_state_id: i32,
    /// The container's expected and planned 27-slot canonical images.
    pub(crate) expected_container: Vec<ItemStack>,
    pub(crate) updated_container: Vec<ItemStack>,
    /// The player whose inventory moves with this deposit, or `None` when the
    /// second participant is inside the receipt.
    pub(crate) player: Option<WarehousePlayerParticipant>,
    /// The encoded plugin operation receipt that rides the container's own
    /// world-journal decision.
    pub(crate) receipt: Vec<u8>,
}

/// Result of one server-owned warehouse deposit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WarehouseTransferOutcome {
    /// The container and the actor's inventory committed together and the
    /// container's after-image and the receipt are journaled under this id.
    Committed { decision_id: u64 },
    /// The player fence moved before the owner turn: recovery pending, a
    /// different inventory or a different carried item.
    StalePlayer,
    /// The container fence moved: its state id, or its authoritative slots.
    StaleContainer,
    /// The container is not a loaded container at the expected position.
    MissingContainer,
}

/// Result of one session-side owned inventory decision.
pub(crate) enum OwnedInventoryCommit {
    /// The request never reached a durable decision; the caller replies with
    /// this typed rejection and performs no effect.
    Rejected(ScriptOperationOutcome),
    /// The decision was committed and its receipt is durable.
    Committed,
}

/// Canonical fingerprint of one owned inventory mutation, matching the plugin
/// storage operation identity rules: a repeated operation id with the same
/// canonical operation replays its stored result, a different one conflicts.
pub(crate) fn owned_inventory_fingerprint(operation: &ScriptOperation) -> [u8; 32] {
    let mut hash = Sha256::new();
    serde_json::to_writer(&mut hash, operation).expect("canonical script operation serializes");
    hash.finalize().into()
}

/// Every endpoint one transfer request names, in canonical order.
pub(crate) fn transfer_endpoints(
    transfers: &[ScriptOwnedItemTransfer],
) -> std::collections::BTreeSet<ScriptInventoryEndpoint> {
    transfers
        .iter()
        .flat_map(|transfer| [transfer.source.clone(), transfer.destination.clone()])
        .collect()
}

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
        ScriptInventoryEndpoint::ResidentEquipment { .. }
            if inventory.len() == usize::from(mc_script::MAX_RESIDENT_EQUIPMENT_SLOTS) =>
        {
            (0, inventory)
        }
        ScriptInventoryEndpoint::ResidentCarry { .. }
            if inventory.len() == usize::from(mc_script::MAX_RESIDENT_CARRY_SLOTS) =>
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

/// One planned worker deposit into one bound container.
#[derive(Debug, Clone)]
pub(crate) struct WarehouseDepositPlan {
    /// The source endpoint's slots after the move.
    pub(crate) updated_source: Vec<ItemStack>,
    /// The destination endpoint's slots after the move.
    pub(crate) updated_destination: Vec<ItemStack>,
    /// The resource ids that entered the destination, with their unit counts.
    pub(crate) moved: BTreeMap<String, u64>,
}

/// Plan one bounded move between a worker's own endpoint and a bound container,
/// in either direction.
///
/// Up to `limit` units move out of the source into the destination in slot
/// order, merging into compatible partial stacks and filling free slots. Every
/// step is applied through [`plan_owned_item_transfers`], so a planned move can
/// never state a stack that planner would reject: what the destination cannot
/// take stays where it was. When `item` names a resource, only stacks of that
/// item are considered - a withdrawal takes what the caller asked for instead of
/// whatever the source happens to hold first.
///
/// A destination with no room for a source that holds something answers
/// [`ScriptOperationFailure::Capacity`], and a source that holds nothing to move
/// answers [`ScriptOperationFailure::InsufficientItems`] - including a filter
/// no source stack matches, so the caller's `no_storage` and `missing_input`
/// pauses stay distinct.
#[allow(clippy::too_many_arguments)]
pub(crate) fn plan_warehouse_deposit(
    source: &ScriptInventoryEndpoint,
    destination: &ScriptInventoryEndpoint,
    source_slots: &[ItemStack],
    destination_slots: &[ItemStack],
    limit: u64,
    items: &ItemRegistry,
    item_facts: &ItemFactsTable,
    item: Option<&str>,
) -> Result<WarehouseDepositPlan, ScriptOperationFailure> {
    if limit == 0 {
        return Err(ScriptOperationFailure::InvalidRequest);
    }
    let mut working = BTreeMap::from([
        (source.clone(), source_slots.to_vec()),
        (destination.clone(), destination_slots.to_vec()),
    ]);
    let mut moved: BTreeMap<String, u64> = BTreeMap::new();
    let mut remaining = limit;
    let mut filter_matches = false;
    'source: for source_slot in 0..source_slots.len() {
        let source_index =
            u8::try_from(source_slot).map_err(|_| ScriptOperationFailure::InvalidRequest)?;
        loop {
            if remaining == 0 {
                break 'source;
            }
            let stack = slot(&working, source, source_index)?.clone();
            if stack.is_empty() {
                continue 'source;
            }
            let name = items
                .name_of(stack.item_id)
                .ok_or(ScriptOperationFailure::InvalidRequest)?
                .as_str()
                .to_owned();
            if item.is_some_and(|wanted| wanted != name) {
                // A filtered move takes the named item only; a stack it did not
                // ask for stays where it is and the next slot is tried.
                continue 'source;
            }
            filter_matches = true;
            let max_stack = item_max_stack(item_facts, items, &stack);
            // The first destination slot with room for this stack decides how
            // much of it moves; a destination that offers none leaves the rest
            // where it was.
            let mut filled = false;
            for destination_slot in 0..destination_slots.len() {
                let destination_index = u8::try_from(destination_slot)
                    .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
                let room = deposit_room(
                    &stack,
                    slot(&working, destination, destination_index)?,
                    max_stack,
                );
                let count = room
                    .min(u32::try_from(stack.count).unwrap_or(u32::MAX))
                    .min(u32::try_from(remaining).unwrap_or(u32::MAX));
                if count == 0 {
                    continue;
                }
                let transfer = ScriptOwnedItemTransfer::new(
                    source.clone(),
                    source_index,
                    destination.clone(),
                    destination_index,
                    count,
                );
                let Ok(updated) = plan_owned_item_transfers(
                    std::slice::from_ref(&transfer),
                    &working,
                    items,
                    item_facts,
                ) else {
                    // The planner refuses this destination for this stack; the
                    // next slot may still take it.
                    continue;
                };
                working = updated;
                *moved.entry(name.clone()).or_default() += u64::from(count);
                remaining -= u64::from(count);
                filled = true;
                break;
            }
            if !filled {
                // Nothing in the destination takes this stack, so it stays where
                // it was and the next one is tried: a destination that still has
                // room for a later stack must not stall the whole move on the
                // first one it cannot take.
                continue 'source;
            }
        }
    }
    if moved.is_empty() {
        // The source held the named item and the destination could not take it
        // (capacity); otherwise there was nothing of what the caller asked for
        // (a missing input, which a filter makes exact).
        let source_holds = filter_matches
            || (item.is_none() && source_slots.iter().any(|stack| !stack.is_empty()));
        return Err(if source_holds {
            ScriptOperationFailure::Capacity
        } else {
            ScriptOperationFailure::InsufficientItems
        });
    }
    Ok(WarehouseDepositPlan {
        updated_source: working
            .remove(source)
            .expect("planned source inventory remains"),
        updated_destination: working
            .remove(destination)
            .expect("planned destination inventory remains"),
        moved,
    })
}

/// Room one container slot offers one moving stack.
///
/// An empty slot takes a whole stack; an occupied slot takes the rest of a
/// compatible one. This is exactly the predicate the transfer planner applies
/// to a destination, so a planned move is never one it would refuse.
fn deposit_room(stack: &ItemStack, destination: &ItemStack, max_stack: i32) -> u32 {
    let room = if destination.is_empty() {
        max_stack
    } else if can_stack(stack, destination) {
        max_stack - destination.count
    } else {
        0
    };
    u32::try_from(room.max(0)).unwrap_or(0)
}

/// Plan and apply every transfer against the involved canonical inventories.
/// The request either moves exactly the requested items on all endpoints or
/// leaves `inventories` untouched: nothing is written before the whole plan
/// succeeds, which is what lets the caller publish one recoverable commit.
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

/// Canonical quantity of one resource held in the given inventory window.
pub(crate) fn inventory_resource_stock(
    slots: &[ItemStack],
    items: &ItemRegistry,
    resource_id: &str,
) -> Result<u64, ScriptOperationFailure> {
    let identifier = mc_protocol::codec::Identifier::parse(resource_id.to_owned())
        .map_err(|_| ScriptOperationFailure::InvalidRequest)?;
    let item_id = items
        .id_of(&identifier)
        .ok_or(ScriptOperationFailure::InvalidRequest)?;
    Ok(slots
        .iter()
        .filter(|stack| !stack.is_empty() && stack.item_id == item_id)
        .map(|stack| u64::try_from(stack.count).unwrap_or(0))
        .sum())
}

/// The reservation keeps its claim on the planned stock: a transfer may never
/// drop an endpoint below the quantities other operations still hold. Both the
/// player/resident transfer and the server-owned warehouse deposit check it.
pub(crate) fn reservation_stock_survives(
    planned: &BTreeMap<ScriptInventoryEndpoint, Vec<ItemStack>>,
    reserved: &BTreeMap<ScriptInventoryEndpoint, BTreeMap<String, u64>>,
    items: &ItemRegistry,
) -> bool {
    reserved.iter().all(|(endpoint, quantities)| {
        let Some(slots) = planned.get(endpoint) else {
            return true;
        };
        let window = endpoint_window(endpoint, slots);
        quantities.iter().all(|(resource_id, quantity)| {
            inventory_resource_stock(window, items, resource_id)
                .is_ok_and(|stock| stock >= *quantity)
        })
    })
}

/// Slot window of one endpoint inside its canonical inventory vector.
pub(crate) fn endpoint_window<'a>(
    endpoint: &ScriptInventoryEndpoint,
    slots: &'a [ItemStack],
) -> &'a [ItemStack] {
    match endpoint {
        ScriptInventoryEndpoint::PlayerInventory { .. } => slots.get(9..=44).unwrap_or(&[]),
        ScriptInventoryEndpoint::Warehouse { .. } => slots,
        ScriptInventoryEndpoint::ResidentEquipment { .. } => slots,
        ScriptInventoryEndpoint::ResidentCarry { .. } => slots,
        _ => &[],
    }
}

/// Stable canonical hash of one resource plan; the reservation identity fence.
pub(crate) fn resource_plan_hash(plan: &ScriptInventoryResourcePlan) -> String {
    let mut hash = Sha256::new();
    serde_json::to_writer(&mut hash, plan).expect("resource plan serialization is infallible");
    format!("{:x}", hash.finalize())
}

/// Exact per-resource totals of one work portion plan.
pub(crate) fn resource_plan_totals(
    plan: &ScriptInventoryResourcePlan,
) -> Result<BTreeMap<String, u64>, ScriptOperationFailure> {
    let mut totals: BTreeMap<String, u64> = BTreeMap::new();
    for portion in &plan.portions {
        for material in &portion.materials {
            let total = totals
                .get(&material.resource_id)
                .copied()
                .unwrap_or(0)
                .checked_add(material.quantity)
                .ok_or(ScriptOperationFailure::InvalidRequest)?;
            totals.insert(material.resource_id.clone(), total);
        }
    }
    Ok(totals)
}

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
        ScriptInventoryEndpoint::ResidentEquipment { .. } => {
            index < mc_script::MAX_RESIDENT_EQUIPMENT_SLOTS
        }
        ScriptInventoryEndpoint::ResidentCarry { .. } => {
            index < mc_script::MAX_RESIDENT_CARRY_SLOTS
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
