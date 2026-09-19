//! The owned-inventory family of the contract, converted to and from the
//! server's own DTO.
//!
//! The generated `inventories` interface is a rename of the server's own
//! vocabulary and nothing more: `decode` builds exactly the
//! `mc_script::ScriptOwnedInventoryOperation` the guest described, `encode_result`
//! renames the server's typed result back, and neither invents a value, clamps
//! one to a bound, or substitutes a different container for the one a plugin
//! named. Bounds are the DTO's own: `ScriptOperationRequest::try_new` is what
//! validates and canonicalizes a decoded operation before any owner sees it, so
//! this conversion only builds the DTO's own constructors - a fence whose hash is
//! not 64 lowercase hexadecimal bytes is the guest's own malformed answer and
//! fails the batch there, exactly as an out-of-range slot does.
//!
//! A native variant this contract cannot express - the server's enums are
//! `non_exhaustive` on purpose - is never mapped onto a member that means
//! something else: `encode_result` answers `None` and the answer is dropped
//! rather than delivered with an invented shape.

use crate::bindings::solaris::plugin::inventories as wire;
use mc_script::{
    ScriptDtoError, ScriptInventoryEndpoint, ScriptInventoryExpectedRevision, ScriptInventoryFence,
    ScriptInventoryMaterial, ScriptInventoryResourcePlan, ScriptInventoryWorkPortion,
    ScriptOwnedInventoryOperation, ScriptOwnedInventoryResult, ScriptOwnedItemTransfer,
};

/// One owned-inventory operation, as the server's own DTO.
///
/// The conversion is total where the contract is total: every endpoint, transfer,
/// fence and plan member the guest named is carried unchanged, and the one thing
/// that can refuse is the fence constructor itself - the server's own
/// `ScriptInventoryFence::try_new`, which refuses a revision or a hash the DTO
/// does not admit. Everything else the DTO bounds is re-checked when the host
/// hands the decoded operation to `ScriptOperationRequest::try_new`.
pub(crate) fn decode(
    value: wire::InventoryOperation,
) -> Result<ScriptOwnedInventoryOperation, ScriptDtoError> {
    Ok(match value {
        wire::InventoryOperation::Query(query) => ScriptOwnedInventoryOperation::Query {
            endpoint: endpoint(query.endpoint),
            expected_revision: query.expected_revision,
        },
        wire::InventoryOperation::Transfer(transfer) => ScriptOwnedInventoryOperation::Transfer {
            operation_id: transfer.operation_id,
            actor_id: transfer.actor_id,
            transfers: transfer
                .transfers
                .into_iter()
                .map(owned_item_transfer)
                .collect(),
            expected_revisions: transfer
                .expected_revisions
                .into_iter()
                .map(expected_revision)
                .collect::<Result<Vec<_>, _>>()?,
        },
        wire::InventoryOperation::Reserve(reserve) => ScriptOwnedInventoryOperation::Reserve {
            operation_id: reserve.operation_id,
            endpoint: endpoint(reserve.endpoint),
            resource_plan: resource_plan(reserve.resource_plan),
            expected_revision: fence(reserve.expected_revision)?,
        },
    })
}

/// One owned-inventory result, as the contract names it, or nothing when the
/// contract cannot express the server's own variant.
pub(crate) fn encode_result(value: &ScriptOwnedInventoryResult) -> Option<wire::InventoryResult> {
    Some(match value {
        ScriptOwnedInventoryResult::Snapshot { inventory } => {
            wire::InventoryResult::Snapshot(snapshot(inventory)?)
        }
        ScriptOwnedInventoryResult::Transfer { inventories } => wire::InventoryResult::Transfer(
            inventories
                .iter()
                .map(contract_expected_revision)
                .collect::<Option<Vec<_>>>()?,
        ),
        ScriptOwnedInventoryResult::Reservation { reservation } => {
            wire::InventoryResult::Reservation(reservation_snapshot(reservation)?)
        }
        _ => return None,
    })
}

/// The longest text one owned-inventory record carries, for the staging bound
/// the host checks a callback's answer against.
///
/// A record carries many strings rather than one, and the server's own DTO
/// already bounds every one of them - a 128-byte warehouse handle, a 64-byte
/// resident handle, a 64-byte operation id, 128-byte resource ids - so the whole
/// record is bounded by construction and charging every string here would refuse
/// a record the DTO admits. What this answers is the longest single string the
/// guest put in the record, exactly as the rest of a staged batch is charged its
/// longest string rather than its whole content.
pub(crate) fn max_text_bytes(value: &wire::InventoryOperation) -> usize {
    match value {
        wire::InventoryOperation::Query(query) => endpoint_text_bytes(&query.endpoint),
        wire::InventoryOperation::Transfer(transfer) => {
            let mut longest = transfer.operation_id.len();
            for item in &transfer.transfers {
                longest = longest
                    .max(endpoint_text_bytes(&item.source))
                    .max(endpoint_text_bytes(&item.destination));
            }
            for expected in &transfer.expected_revisions {
                longest = longest
                    .max(endpoint_text_bytes(&expected.endpoint))
                    .max(expected.fence.snapshot_hash.len());
            }
            longest
        }
        wire::InventoryOperation::Reserve(reserve) => {
            let mut longest = reserve
                .operation_id
                .len()
                .max(endpoint_text_bytes(&reserve.endpoint))
                .max(reserve.expected_revision.snapshot_hash.len());
            for portion in &reserve.resource_plan.portions {
                for material in &portion.materials {
                    longest = longest.max(material.resource_id.len());
                }
            }
            longest
        }
    }
}

/// The strings one endpoint carries. A player endpoint is a number and carries
/// none.
fn endpoint_text_bytes(value: &wire::InventoryEndpoint) -> usize {
    match value {
        wire::InventoryEndpoint::PlayerInventory(_) => 0,
        wire::InventoryEndpoint::Warehouse(handle)
        | wire::InventoryEndpoint::ResidentEquipment(handle)
        | wire::InventoryEndpoint::ResidentCarry(handle) => handle.len(),
    }
}

/// One contract endpoint as the server's own.
fn endpoint(value: wire::InventoryEndpoint) -> ScriptInventoryEndpoint {
    match value {
        wire::InventoryEndpoint::PlayerInventory(player_id) => {
            ScriptInventoryEndpoint::PlayerInventory { player_id }
        }
        wire::InventoryEndpoint::Warehouse(handle) => ScriptInventoryEndpoint::Warehouse { handle },
        wire::InventoryEndpoint::ResidentEquipment(handle) => {
            ScriptInventoryEndpoint::ResidentEquipment { handle }
        }
        wire::InventoryEndpoint::ResidentCarry(handle) => {
            ScriptInventoryEndpoint::ResidentCarry { handle }
        }
    }
}

/// One server endpoint as the contract names it, or nothing for a variant this
/// contract does not name yet.
fn contract_endpoint(value: &ScriptInventoryEndpoint) -> Option<wire::InventoryEndpoint> {
    Some(match value {
        ScriptInventoryEndpoint::PlayerInventory { player_id } => {
            wire::InventoryEndpoint::PlayerInventory(*player_id)
        }
        ScriptInventoryEndpoint::Warehouse { handle } => {
            wire::InventoryEndpoint::Warehouse(handle.clone())
        }
        ScriptInventoryEndpoint::ResidentEquipment { handle } => {
            wire::InventoryEndpoint::ResidentEquipment(handle.clone())
        }
        ScriptInventoryEndpoint::ResidentCarry { handle } => {
            wire::InventoryEndpoint::ResidentCarry(handle.clone())
        }
        _ => return None,
    })
}

/// One contract fence as the server's own, through the DTO's own constructor.
fn fence(value: wire::InventoryFence) -> Result<ScriptInventoryFence, ScriptDtoError> {
    ScriptInventoryFence::try_new(value.revision, value.snapshot_hash)
}

/// One contract expected revision as the server's own.
fn expected_revision(
    value: wire::InventoryExpectedRevision,
) -> Result<ScriptInventoryExpectedRevision, ScriptDtoError> {
    Ok(ScriptInventoryExpectedRevision::new(
        endpoint(value.endpoint),
        fence(value.fence)?,
    ))
}

/// One server expected revision as the contract names it.
fn contract_expected_revision(
    value: &ScriptInventoryExpectedRevision,
) -> Option<wire::InventoryExpectedRevision> {
    Some(wire::InventoryExpectedRevision {
        endpoint: contract_endpoint(&value.endpoint)?,
        fence: contract_fence(&value.fence),
    })
}

/// One server fence as the contract names it. A fence the server holds is
/// already a revision and a 64-byte hash, so this is a rename.
fn contract_fence(value: &ScriptInventoryFence) -> wire::InventoryFence {
    wire::InventoryFence {
        revision: value.revision,
        snapshot_hash: value.snapshot_hash.clone(),
    }
}

/// One contract item transfer as the server's own.
fn owned_item_transfer(value: wire::OwnedItemTransfer) -> ScriptOwnedItemTransfer {
    ScriptOwnedItemTransfer::new(
        endpoint(value.source),
        value.source_slot,
        endpoint(value.destination),
        value.destination_slot,
        value.count,
    )
}

/// One contract resource plan as the server's own.
fn resource_plan(value: wire::InventoryResourcePlan) -> ScriptInventoryResourcePlan {
    ScriptInventoryResourcePlan::new(
        value
            .portions
            .into_iter()
            .map(|portion| {
                ScriptInventoryWorkPortion::new(
                    portion.work_units,
                    portion
                        .materials
                        .into_iter()
                        .map(|material| {
                            ScriptInventoryMaterial::new(material.resource_id, material.quantity)
                        })
                        .collect(),
                )
            })
            .collect(),
    )
}

/// One server snapshot as the contract names it.
fn snapshot(
    value: &mc_script::ScriptOwnedInventorySnapshot,
) -> Option<wire::OwnedInventorySnapshot> {
    Some(wire::OwnedInventorySnapshot {
        endpoint: contract_endpoint(&value.endpoint)?,
        fence: contract_fence(&value.fence),
        slots: value
            .slots
            .iter()
            .map(item_slot)
            .collect::<Option<Vec<_>>>()?,
    })
}

/// One server slot as the contract names it.
fn item_slot(value: &mc_script::ScriptInventorySlot) -> Option<wire::InventorySlot> {
    Some(wire::InventorySlot {
        slot: value.slot,
        item: match &value.item {
            Some(value) => Some(item(value)?),
            None => None,
        },
    })
}

/// One server item as the contract names it, components included.
fn item(value: &mc_script::ScriptInventoryItem) -> Option<wire::InventoryItem> {
    Some(wire::InventoryItem {
        resource_id: value.resource_id.clone(),
        count: value.count,
        damage: value.damage,
        enchantments: value
            .enchantments
            .iter()
            .map(|enchantment| wire::InventoryEnchantment {
                resource_id: enchantment.resource_id.clone(),
                level: enchantment.level,
            })
            .collect(),
        custom_name: value.custom_name.clone(),
        item_model: value.item_model.clone(),
    })
}

/// One server reservation as the contract names it.
fn reservation_snapshot(
    value: &mc_script::ScriptInventoryReservationSnapshot,
) -> Option<wire::InventoryReservationSnapshot> {
    Some(wire::InventoryReservationSnapshot {
        reservation_ref: value.reservation_ref.clone(),
        endpoint: contract_endpoint(&value.endpoint)?,
        resource_plan_hash: value.resource_plan_hash.clone(),
        quantities: value
            .quantities
            .iter()
            .map(|quantity| wire::InventoryReservationQuantity {
                resource_id: quantity.resource_id.clone(),
                reserved: quantity.reserved,
                consumed: quantity.consumed,
                returned: quantity.returned,
                remaining: quantity.remaining,
            })
            .collect(),
        bound_to: value.bound_to.clone(),
        released: value.released,
        receipt_watermark: value.receipt_watermark,
    })
}
