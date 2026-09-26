//! Resident and resident-order commands: the durable identity, point-of-interest
//! and work/order calls the settlement flows make.
//!
//! Each helper builds exactly the record `residents.wit` declares and nothing
//! more: the handle, the revision, the work order and the squad order are the
//! plugin's own, and the server's own DTO validator is what refuses a value
//! outside the contract's bounds when the host converts the batch. No helper
//! waits for an answer - every call answers later as
//! [`Event::OperationAnswered`](crate::events::Event::OperationAnswered) with the
//! same `request`, carrying the server's typed resident or resident-order result
//! and, for a mutation, the durable operation id the commit was recorded under.
//!
//! The two names a resident call carries mean different things and neither is a
//! plugin's to invent: `operation_id` is the durable name the server records the
//! commit under, so repeating a call with byte-identical content replays that
//! outcome and applies nothing, while `request` is correlation only. The handles,
//! the spawn site token and the POI handles are the opaque strings the server
//! already answered this plugin with - a plugin never derives one, and a handle
//! that belongs to another plugin is refused rather than resolved.

use crate::{domain_operations, inventories, residents, Command};

/// Adopt the live villager one entity uuid names.
///
/// The uuid is the server's canonical dashed lowercase string of a live villager
/// the claiming actor can reach: the server answers `not_found` for an entity it
/// does not hold, `forbidden` for one further away than the claim distance or for
/// an actor whose session is not in the simulated dimension, and `capacity` when
/// the plugin already holds its living residents. `expected_entity_revision` is
/// `0` for a first claim and the resident's current revision when this plugin
/// already holds it, so a claim that re-reads a resident it owns is answered with
/// that resident rather than adopting a second one.
///
/// The answer is the resident's snapshot, whose handle the server minted from
/// this plugin and the entity - the handle a plugin sends afterwards, never an
/// entity id of its own.
#[must_use]
pub fn claim_resident(
    request: &str,
    operation_id: &str,
    actor_id: u64,
    entity_uuid: &str,
    expected_entity_revision: u64,
) -> Command {
    operation(
        request,
        residents::ResidentOperation::Claim(residents::ResidentClaim {
            operation_id: operation_id.to_owned(),
            actor_id,
            entity_uuid: entity_uuid.to_owned(),
            expected_entity_revision,
        }),
    )
}

/// Materialise the resident one reserved site is waiting for.
///
/// The token is the opaque `spawn-site-token` a resident site reservation
/// answered with, and `kind` is the contract's closed kind vocabulary - the
/// server's own villager. The server materialises that entity at the reserved
/// point of interest, so the spawn is refused `not_found` for a reservation this
/// plugin does not hold or one already released, `capacity` for one already
/// consumed, `unloaded` while that cell's chunk is not loaded, and `blocked` for
/// a cell that cannot hold a standing body. The reservation is consumed by the
/// commit and by nothing else: a refused spawn leaves it in place, so a plugin
/// retries the same reservation instead of minting a second one.
///
/// The answer is the new resident's snapshot. Only a commit creates the resident,
/// so a plugin must not read a refusal as "the resident exists".
#[must_use]
pub fn spawn_resident(
    request: &str,
    operation_id: &str,
    spawn_site_token: &str,
    kind: residents::ResidentKind,
) -> Command {
    operation(
        request,
        residents::ResidentOperation::Spawn(residents::ResidentSpawn {
            operation_id: operation_id.to_owned(),
            spawn_site_token: spawn_site_token.to_owned(),
            profile: residents::ResidentProfile { kind },
        }),
    )
}

/// Replace the home, work and meeting points of interest of one resident.
///
/// All three are sent in one call, so a binding is never half applied: an absent
/// value unbinds that POI, and the server records the whole set or nothing.
/// `expected_revision` is the revision of the resident snapshot the plugin read;
/// a plugin that sends a stale one is refused `stale_revision` and the binding it
/// read stays exactly as it was, which is why a revision that was refused can be
/// sent again unchanged.
///
/// The answer is the resident's snapshot as the server now holds it, whose
/// `pois` are the ones this call committed.
#[must_use]
pub fn set_resident_pois(
    request: &str,
    operation_id: &str,
    handle: &str,
    home_poi: Option<&str>,
    work_poi: Option<&str>,
    meeting_poi: Option<&str>,
    expected_revision: u64,
) -> Command {
    operation(
        request,
        residents::ResidentOperation::SetPois(residents::ResidentSetPois {
            operation_id: operation_id.to_owned(),
            handle: handle.to_owned(),
            home_poi: home_poi.map(str::to_owned),
            work_poi: work_poi.map(str::to_owned),
            meeting_poi: meeting_poi.map(str::to_owned),
            expected_revision,
        }),
    )
}

/// Ask core to spend real warehouse supplies and heal one wounded resident.
#[must_use]
pub fn treat_resident(
    request: &str,
    operation_id: &str,
    handle: &str,
    expected_revision: u64,
    source: inventories::InventoryEndpoint,
    material: inventories::InventoryMaterial,
    heal_milli: u32,
) -> Command {
    operation(
        request,
        residents::ResidentOperation::Treat(residents::ResidentTreat {
            operation_id: operation_id.to_owned(),
            handle: handle.to_owned(),
            expected_revision,
            source,
            material,
            heal_milli,
        }),
    )
}

/// Read the server's current owner-scoped resident snapshots. An explicit
/// handle that is absent or foreign is refused; this read observes death
/// before answering, so role policy cannot equip a tombstoned resident.
/// An empty list requests one bounded page instead.
#[must_use]
pub fn query_residents(request: &str, handles: &[String], cursor: Option<&str>) -> Command {
    operation(
        request,
        residents::ResidentOperation::Query(residents::ResidentQuery {
            handles: handles.to_vec(),
            cursor: cursor.map(str::to_owned),
        }),
    )
}

/// Give one resident a bounded work order.
///
/// `work` is the closed work-order union the current settlement flows use - a
/// harvest, cut-tree, mine, fish, tend-livestock, haul, craft or construct order -
/// and `work_units` is how much of it this call may commit, at least one and at
/// most `mc_script::MAX_WORK_UNITS` (4096). The engine reads the live world and
/// decides what the worker can actually do, so the answer is not a promise: a
/// worker without its tool pauses `missing_tool`, one whose container is empty
/// pauses `missing_input`, one whose chunk is not loaded pauses `unloaded`, and
/// the committed item changes the engine reports are the real ones it made.
///
/// `expected_revision` is the revision of the resident's own work record, which
/// is `0` for a resident this plugin has never given work. A stale one is refused
/// `stale_revision` with nothing applied, which is what makes an assignment safe
/// to retry against the revision the last answer named.
#[must_use]
pub fn assign_resident_work(
    request: &str,
    operation_id: &str,
    handle: &str,
    work: residents::WorkOrder,
    work_units: u64,
    expected_revision: u64,
) -> Command {
    order(
        request,
        residents::ResidentOrderOperation::AssignWork(residents::AssignWork {
            operation_id: operation_id.to_owned(),
            handle: handle.to_owned(),
            work,
            work_units,
            expected_revision,
        }),
    )
}

/// Stop what one resident was told to do.
///
/// The answer is the cancelled assignment's durable revision, which is the
/// revision this resident's work record now holds: a plugin sends that back for
/// the next call about the same resident. A stale `expected_revision` is refused
/// rather than cancelling work another decision already moved.
#[must_use]
pub fn cancel_resident_work(
    request: &str,
    operation_id: &str,
    handle: &str,
    expected_revision: u64,
) -> Command {
    order(
        request,
        residents::ResidentOrderOperation::CancelWork(residents::CancelWork {
            operation_id: operation_id.to_owned(),
            handle: handle.to_owned(),
            expected_revision,
        }),
    )
}

/// Give every named member the same squad order.
///
/// `handles` and `expected_order_revisions` are two parallel lists: the durable
/// handles this plugin holds and the order revision each member is fenced on,
/// one entry per handle in the same order. The server pairs, sorts and dedups
/// them, so one operation id always fingerprints identically. A member whose
/// revision has moved, that is not this plugin's, or whose engine admission does
/// not commit refuses the batch with `blocked` and answers one outcome per
/// member - every one of them `stale_revision` - which is why the answer carries
/// member detail even when nothing was applied.
///
/// `order` is the closed squad-order union: follow, move, hold, patrol, garrison,
/// attack or retreat. The server plans each member, so the answer reports what
/// happened to each one - `applied` with its formation slot, or the state it
/// could not be given the order in - plus the combat it committed, each event
/// named by the server so experience is awarded exactly once.
#[must_use]
pub fn issue_resident_order(
    request: &str,
    operation_id: &str,
    handles: Vec<String>,
    expected_order_revisions: Vec<u64>,
    order: residents::Order,
) -> Command {
    self::order(
        request,
        residents::ResidentOrderOperation::IssueOrder(residents::IssueOrder {
            operation_id: operation_id.to_owned(),
            handles,
            expected_order_revisions,
            order,
        }),
    )
}

/// Cancel the squad order every named member holds.
///
/// The member list is the same two parallel lists [`issue_resident_order`] takes,
/// with each member's current order revision. An accepted batch answers one
/// `applied` outcome per member - a cancellation carries the member's batch
/// ordinal as its slot, never a formation slot - and clears the order; a batch
/// the server refuses answers `blocked` with the same per-member detail an
/// issuance does.
#[must_use]
pub fn cancel_resident_order(
    request: &str,
    operation_id: &str,
    handles: Vec<String>,
    expected_order_revisions: Vec<u64>,
) -> Command {
    order(
        request,
        residents::ResidentOrderOperation::CancelOrder(residents::CancelOrder {
            operation_id: operation_id.to_owned(),
            handles,
            expected_order_revisions,
        }),
    )
}

/// Move a serving resident through `demobilizing → civilian`.
///
/// The first fenced call cancels any active goals and work without moving
/// items. A caller may use a separate fenced [`crate::transfer_owned_items`]
/// call according to its own item policy; the second fenced call records the
/// civilian assignment. Both decisions retain the same resident handle and
/// housing. A resident equipped before their first military order may still
/// have a civilian order assignment. `expected_revision` is the resident
/// order/gear fence, not the independently versioned resident-identity snapshot.
#[must_use]
pub fn demobilize_resident(
    request: &str,
    operation_id: &str,
    handle: &str,
    expected_revision: u64,
) -> Command {
    order(
        request,
        residents::ResidentOrderOperation::Demobilize(residents::Demobilize {
            operation_id: operation_id.to_owned(),
            handle: handle.to_owned(),
            expected_revision,
        }),
    )
}

/// A serving guard must physically reach a living routing combatant. The
/// accepted capture interrupts combat without changing resident or gear identity.
#[must_use]
pub fn capture_resident(
    request: &str,
    operation_id: &str,
    handle: &str,
    custodian: &str,
    expected_revision: u64,
) -> Command {
    order(
        request,
        residents::ResidentOrderOperation::Capture(residents::Capture {
            operation_id: operation_id.to_owned(),
            handle: handle.to_owned(),
            custodian: custodian.to_owned(),
            expected_revision,
        }),
    )
}

/// One resident record wrapped in the single operation envelope every family
/// shares.
fn operation(request: &str, record: residents::ResidentOperation) -> Command {
    Command::Operation(domain_operations::OperationRequest {
        request: request.to_owned(),
        operation: domain_operations::DomainOperation::Resident(record),
    })
}

/// One resident work or squad record wrapped in the same envelope, under its own
/// family: the two are separate names because the server's own unions differ, and
/// a work order sent as a resident call would be a different operation.
fn order(request: &str, record: residents::ResidentOrderOperation) -> Command {
    Command::Operation(domain_operations::OperationRequest {
        request: request.to_owned(),
        operation: domain_operations::DomainOperation::ResidentOrder(record),
    })
}
