//! Owned-inventory commands: the three operations the used settlement flows
//! need against the endpoints the server already owns.
//!
//! Each helper builds exactly the record `inventories.wit` declares and nothing
//! more: the endpoint, the revision and the items are the plugin's own, and the
//! server's own DTO validator is what refuses a value outside the contract's
//! bounds when the host converts the batch. No helper waits for an answer - a
//! query, a transfer and a reservation each answer later as
//! [`Event::OperationAnswered`](crate::events::Event::OperationAnswered) with the
//! same `request`.

use crate::{domain_operations, inventories, Command};

/// Read one owned endpoint, optionally fenced to a revision.
///
/// The answer is the endpoint's whole canonical snapshot: every occupied and
/// empty slot with the items and their components exactly as the owner holds
/// them, plus the `inventory-fence` a later transfer or reservation fences
/// against. `expected-revision` is `None` for an unfenced read; naming a revision
/// the endpoint has moved past is answered `stale-revision` instead of the
/// snapshot.
///
/// A query carries no durable operation id: it changes nothing and creates no
/// receipt, so the answer's `operation-id` is absent rather than invented and a
/// plugin must not read it as one. The endpoint may be any of the four - a
/// player's inventory, a bound warehouse, or one durable resident's equipment or
/// carry - and a handle the owner does not hold is answered `not-found` rather
/// than with an empty inventory.
#[must_use]
pub fn query_owned_inventory(
    request: &str,
    endpoint: inventories::InventoryEndpoint,
    expected_revision: Option<u64>,
) -> Command {
    operation(
        request,
        inventories::InventoryOperation::Query(inventories::InventoryQuery {
            endpoint,
            expected_revision,
        }),
    )
}

/// Move items between owned endpoints under one durable operation id.
///
/// A player endpoint requires the exact participating runtime player id:
/// another player's inventory is `forbidden`, and a disconnected session is
/// `not-found`. `actor_id` is zero only for a bound warehouse and this
/// plugin's resident equipment/carry: stock may be issued or returned without
/// a player participant. `expected_revisions` is one fence per distinct
/// endpoint the transfers name - the fences the plugin read with
/// [`query_owned_inventory`] - and a fence that has moved is refused
/// `stale-revision`, never applied against the image the plugin did not see.
///
/// `operation_id` is the durable name the server records the commit under: a
/// transfer the plugin repeats with byte-identical content replays the recorded
/// outcome and applies nothing, which is what makes retrying after a lost answer
/// safe, while reusing the id for different content is refused
/// `operation-conflict`. The answer reports the committed server-owned endpoint
/// fences; a participating player's fence is read again with a query after its
/// world-journal decision id has been assigned.
///
/// The items move as the stacks they are, components included: a transfer never
/// substitutes a resource-only count for an item the owner holds.
#[must_use]
pub fn transfer_owned_items(
    request: &str,
    operation_id: &str,
    actor_id: u64,
    transfers: Vec<inventories::OwnedItemTransfer>,
    expected_revisions: Vec<inventories::InventoryExpectedRevision>,
) -> Command {
    operation(
        request,
        inventories::InventoryOperation::Transfer(inventories::InventoryTransfer {
            operation_id: operation_id.to_owned(),
            actor_id,
            transfers,
            expected_revisions,
        }),
    )
}

/// Reserve the materials of one resource plan against one owned endpoint.
///
/// The reservation is durable: the server records it under `operation_id`, holds
/// the plan's quantities against the endpoint so a later transfer cannot draw
/// below them, and answers the whole `inventory-reservation-snapshot` - the
/// opaque `reservation-ref`, the plan hash and the reserved, consumed, returned
/// and remaining units of every resource. A reservation repeats under the same
/// operation id with byte-identical content as a replay, exactly as a transfer
/// does.
///
/// `expected_revision` is the endpoint's fence, read with
/// [`query_owned_inventory`]: a reservation against a fence that has moved is
/// refused `stale-revision` rather than held against the wrong image, and a plan
/// the endpoint cannot cover is refused `insufficient-items`. A warehouse
/// reservation is admitted only through the caller's bound warehouse handle;
/// core locks the resolved physical container for the durable decision.
#[must_use]
pub fn reserve_inventory_items(
    request: &str,
    operation_id: &str,
    endpoint: inventories::InventoryEndpoint,
    resource_plan: inventories::InventoryResourcePlan,
    expected_revision: inventories::InventoryFence,
) -> Command {
    operation(
        request,
        inventories::InventoryOperation::Reserve(inventories::InventoryReserve {
            operation_id: operation_id.to_owned(),
            endpoint,
            resource_plan,
            expected_revision,
        }),
    )
}

/// One owned-inventory record wrapped in the single operation envelope every
/// family shares.
fn operation(request: &str, record: inventories::InventoryOperation) -> Command {
    Command::Operation(domain_operations::OperationRequest {
        request: request.to_owned(),
        operation: domain_operations::DomainOperation::Inventory(record),
    })
}
