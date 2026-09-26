//! The resident and resident-order families of the contract, converted to and
//! from the server's own DTOs.
//!
//! The generated `residents` interface is a rename of the server's own
//! vocabulary and nothing more: `decode`/`decode_order` build exactly the
//! `mc_script::ScriptResidentOperation`/`ScriptResidentOrderOperation` the guest
//! described, `encode_result`/`encode_order_result` rename the server's typed
//! results back, and none of them invents a value, clamps one to a bound, or
//! substitutes a different resident, endpoint or target for the one a plugin
//! named. Bounds stay the DTO's own: `ScriptOperationRequest::try_new` is what
//! validates and canonicalizes a decoded operation before any owner sees it -
//! including the parallel handle/revision lists, which it sorts and dedups so one
//! operation id always fingerprints identically - so this conversion only calls
//! the DTO's own constructors.
//!
//! A native variant this contract cannot express - the server's enums are
//! `non_exhaustive` on purpose - is never mapped onto a member that means
//! something else: the `encode_*` functions answer `None` and the answer is
//! dropped rather than delivered with an invented shape. A refusal is not such a
//! variant: `resident_order_batch_refusal` answers a `blocked` refusal whose
//! payload carries one per-member state per handle, and that payload is encoded
//! here like any other, so a plugin reads why every member was refused instead of
//! losing the batch detail behind a bare failure.

use crate::bindings::solaris::plugin::inventories as wire_inventories;
use crate::bindings::solaris::plugin::residents as wire;
use mc_script::{
    ScriptBlockPosition, ScriptCombatEvent, ScriptDemobilizeResult, ScriptDemobilizeState,
    ScriptDtoError, ScriptEngagementPolicy, ScriptFormation, ScriptFormationKind,
    ScriptHostileCategory, ScriptInventoryEndpoint, ScriptInventoryMaterial, ScriptItemChange,
    ScriptOrderMemberOutcome, ScriptOrderMemberState, ScriptOrderTarget, ScriptOrderTargetRef,
    ScriptResidentItemSummary, ScriptResidentKind, ScriptResidentLifecycle,
    ScriptResidentLoadedState, ScriptResidentOperation, ScriptResidentOrder,
    ScriptResidentOrderOperation, ScriptResidentOrderResult, ScriptResidentProfile,
    ScriptResidentResult, ScriptResidentSnapshot, ScriptResidentWorkOrder, ScriptWorkArea,
    ScriptWorkAssignment, ScriptWorkPauseReason, ScriptWorkState,
};

/// One resident call, as the server's own DTO.
///
/// Every field the guest named is carried unchanged; the only thing this builds
/// is the DTO's own profile constructor, which is the one place the contract's
/// closed kind vocabulary becomes the server's.
pub(crate) fn decode(
    value: wire::ResidentOperation,
) -> Result<ScriptResidentOperation, ScriptDtoError> {
    Ok(match value {
        wire::ResidentOperation::Claim(claim) => ScriptResidentOperation::Claim {
            operation_id: claim.operation_id,
            actor_id: claim.actor_id,
            entity_uuid: claim.entity_uuid,
            expected_entity_revision: claim.expected_entity_revision,
        },
        wire::ResidentOperation::Spawn(spawn) => ScriptResidentOperation::Spawn {
            operation_id: spawn.operation_id,
            spawn_site_token: spawn.spawn_site_token,
            profile: ScriptResidentProfile::new(kind(spawn.profile.kind)),
        },
        wire::ResidentOperation::SetPois(pois) => ScriptResidentOperation::SetPois {
            operation_id: pois.operation_id,
            handle: pois.handle,
            home_poi: pois.home_poi,
            work_poi: pois.work_poi,
            meeting_poi: pois.meeting_poi,
            expected_revision: pois.expected_revision,
        },
        wire::ResidentOperation::Query(query) => ScriptResidentOperation::Query {
            handles: query.handles,
            cursor: query.cursor,
        },
        wire::ResidentOperation::Treat(treat) => ScriptResidentOperation::Treat {
            operation_id: treat.operation_id,
            handle: treat.handle,
            expected_revision: treat.expected_revision,
            source: endpoint(treat.source),
            material: ScriptInventoryMaterial::new(
                treat.material.resource_id,
                treat.material.quantity,
            ),
            heal_milli: treat.heal_milli,
        },
    })
}

/// One resident work or squad call, as the server's own DTO.
///
/// The two parallel lists an order carries - the durable handles and the order
/// revision each member is fenced on - are handed over as they arrived, in the
/// same order and the same length: the DTO's own canonicalization is what pairs
/// them, and a conversion that paired them itself would be a second, silent
/// interpretation of the guest's answer.
pub(crate) fn decode_order(
    value: wire::ResidentOrderOperation,
) -> Result<ScriptResidentOrderOperation, ScriptDtoError> {
    Ok(match value {
        wire::ResidentOrderOperation::AssignWork(assign) => {
            ScriptResidentOrderOperation::AssignWork {
                operation_id: assign.operation_id,
                handle: assign.handle,
                work: work_order(assign.work),
                work_units: assign.work_units,
                expected_revision: assign.expected_revision,
            }
        }
        wire::ResidentOrderOperation::CancelWork(cancel) => {
            ScriptResidentOrderOperation::CancelWork {
                operation_id: cancel.operation_id,
                handle: cancel.handle,
                expected_revision: cancel.expected_revision,
            }
        }
        wire::ResidentOrderOperation::IssueOrder(issue) => {
            ScriptResidentOrderOperation::IssueOrder {
                operation_id: issue.operation_id,
                handles: issue.handles,
                expected_order_revisions: issue.expected_order_revisions,
                order: order(issue.order),
            }
        }
        wire::ResidentOrderOperation::CancelOrder(cancel) => {
            ScriptResidentOrderOperation::CancelOrder {
                operation_id: cancel.operation_id,
                handles: cancel.handles,
                expected_order_revisions: cancel.expected_order_revisions,
            }
        }
        wire::ResidentOrderOperation::Demobilize(demobilize) => {
            ScriptResidentOrderOperation::Demobilize {
                operation_id: demobilize.operation_id,
                handle: demobilize.handle,
                expected_revision: demobilize.expected_revision,
            }
        }
        wire::ResidentOrderOperation::Capture(capture) => ScriptResidentOrderOperation::Capture {
            operation_id: capture.operation_id,
            handle: capture.handle,
            custodian: capture.custodian,
            expected_revision: capture.expected_revision,
        },
    })
}

/// One resident result, as the contract names it. The page is the core's
/// owner-scoped, bounded query response; it must not be renamed into a single
/// resident snapshot or silently dropped when the caller reads lifecycle.
pub(crate) fn encode_result(value: &ScriptResidentResult) -> Option<wire::ResidentResult> {
    Some(match value {
        ScriptResidentResult::Snapshot { resident } => {
            wire::ResidentResult::Snapshot(snapshot(resident)?)
        }
        ScriptResidentResult::Page { residents, cursor } => {
            wire::ResidentResult::Page(wire::ResidentPage {
                residents: residents.iter().map(snapshot).collect::<Option<Vec<_>>>()?,
                cursor: cursor.clone(),
            })
        }
        ScriptResidentResult::Treated { handle } => wire::ResidentResult::Treated(handle.clone()),
        _ => return None,
    })
}

/// One resident work or squad result, as the contract names it, or nothing when
/// the contract cannot express the server's own variant.
///
/// Every variant the current operations can commit is named here, refusals
/// included: a refused order still carries its per-member outcomes, so a plugin
/// reads which member was blocked, unloaded or stale instead of a bare refusal.
pub(crate) fn encode_order_result(
    value: &ScriptResidentOrderResult,
) -> Option<wire::ResidentOrderResult> {
    Some(match value {
        ScriptResidentOrderResult::Work { assignment } => {
            wire::ResidentOrderResult::Work(work_assignment(assignment)?)
        }
        ScriptResidentOrderResult::WorkCancelled { handle, revision } => {
            wire::ResidentOrderResult::WorkCancelled(wire::WorkCancelled {
                handle: handle.clone(),
                revision: *revision,
            })
        }
        ScriptResidentOrderResult::Order {
            order_revision,
            members,
            combat,
        } => wire::ResidentOrderResult::Order(wire::OrderResult {
            order_revision: *order_revision,
            members: members
                .iter()
                .map(member_outcome)
                .collect::<Option<Vec<_>>>()?,
            combat: combat.iter().map(combat_event).collect(),
        }),
        ScriptResidentOrderResult::OrderCancelled {
            order_revision,
            members,
        } => wire::ResidentOrderResult::OrderCancelled(wire::OrderCancelled {
            order_revision: *order_revision,
            members: members
                .iter()
                .map(member_outcome)
                .collect::<Option<Vec<_>>>()?,
        }),
        ScriptResidentOrderResult::Demobilized { resident } => {
            wire::ResidentOrderResult::Demobilized(demobilize_result(resident)?)
        }
        ScriptResidentOrderResult::Captured {
            handle,
            custodian,
            revision,
        } => wire::ResidentOrderResult::Captured(wire::CaptureResult {
            handle: handle.clone(),
            custodian: custodian.clone(),
            revision: *revision,
        }),
        _ => return None,
    })
}

/// The longest text one resident record carries, for the staging bound the host
/// checks a callback's answer against.
///
/// A record carries several strings rather than one, and the server's own DTO
/// already bounds every one of them - a 64-byte operation id, a 64-byte resident
/// handle, a 64-byte spawn site token, 128-byte POI handles - so the whole record
/// is bounded by construction and charging every string here would refuse a
/// record the DTO admits. What this answers is the longest single string the
/// guest put in the record, exactly as the rest of a staged batch is charged its
/// longest string rather than its whole content.
pub(crate) fn max_text_bytes(value: &wire::ResidentOperation) -> usize {
    match value {
        wire::ResidentOperation::Claim(claim) => {
            claim.entity_uuid.len().max(claim.operation_id.len())
        }
        wire::ResidentOperation::Spawn(spawn) => {
            spawn.operation_id.len().max(spawn.spawn_site_token.len())
        }
        wire::ResidentOperation::SetPois(pois) => {
            let mut longest = pois.operation_id.len().max(pois.handle.len());
            for poi in [&pois.home_poi, &pois.work_poi, &pois.meeting_poi]
                .into_iter()
                .flatten()
            {
                longest = longest.max(poi.len());
            }
            longest
        }
        wire::ResidentOperation::Query(query) => query
            .handles
            .iter()
            .map(String::len)
            .chain(query.cursor.as_ref().map(String::len))
            .max()
            .unwrap_or(0),
        wire::ResidentOperation::Treat(treat) => treat
            .operation_id
            .len()
            .max(treat.handle.len())
            .max(endpoint_text_bytes(&treat.source))
            .max(treat.material.resource_id.len()),
    }
}

/// The longest text one resident work or squad record carries, for the same
/// staging bound.
pub(crate) fn max_order_text_bytes(value: &wire::ResidentOrderOperation) -> usize {
    match value {
        wire::ResidentOrderOperation::AssignWork(assign) => assign
            .operation_id
            .len()
            .max(assign.handle.len())
            .max(work_text_bytes(&assign.work)),
        wire::ResidentOrderOperation::CancelWork(cancel) => {
            cancel.operation_id.len().max(cancel.handle.len())
        }
        wire::ResidentOrderOperation::IssueOrder(issue) => {
            let mut longest = handle_text_bytes(&issue.operation_id, &issue.handles);
            longest = longest.max(order_text_bytes(&issue.order));
            longest
        }
        wire::ResidentOrderOperation::CancelOrder(cancel) => {
            handle_text_bytes(&cancel.operation_id, &cancel.handles)
        }
        wire::ResidentOrderOperation::Demobilize(demobilize) => {
            demobilize.operation_id.len().max(demobilize.handle.len())
        }
        wire::ResidentOrderOperation::Capture(capture) => capture
            .operation_id
            .len()
            .max(capture.handle.len())
            .max(capture.custodian.len()),
    }
}

/// The longest of an operation id and the durable handles of one batch.
fn handle_text_bytes(operation_id: &str, handles: &[String]) -> usize {
    handles.iter().fold(operation_id.len(), |longest, handle| {
        longest.max(handle.len())
    })
}

/// The longest string one work order carries.
fn work_text_bytes(value: &wire::WorkOrder) -> usize {
    match value {
        wire::WorkOrder::Harvest(work)
        | wire::WorkOrder::CutTree(work)
        | wire::WorkOrder::Mine(work)
        | wire::WorkOrder::Fish(work) => work.tool.len().max(work.area.dimension.len()),
        wire::WorkOrder::TendLivestock(work) => work.feed.len().max(work.area.dimension.len()),
        wire::WorkOrder::Haul(haul) => endpoint_text_bytes(&haul.source)
            .max(endpoint_text_bytes(&haul.destination))
            .max(haul.item.as_ref().map_or(0, String::len)),
        wire::WorkOrder::Craft(craft) => craft.recipe.len().max(craft.station.dimension.len()),
        wire::WorkOrder::Construct(construct) => {
            construct.structure_id.len().max(construct.stage.len())
        }
    }
}

/// The longest string one squad order carries.
///
/// A formation, a heading, a spacing and an anchor are numbers and closed enums,
/// so the orders built only from them carry no text at all and answer zero. The
/// rest name resources, posts or target references, which the server's own DTO
/// bounds.
fn order_text_bytes(value: &wire::Order) -> usize {
    match value {
        wire::Order::Follow(_)
        | wire::Order::Hold(_)
        | wire::Order::Patrol(_)
        | wire::Order::Retreat(_) => 0,
        wire::Order::Move(move_order) => move_order.dimension.len(),
        wire::Order::Garrison(garrison) => garrison
            .posts
            .iter()
            .fold(0, |longest, post| longest.max(post.len())),
        wire::Order::Attack(attack) => {
            let mut longest = attack
                .targets
                .iter()
                .fold(0, |longest, target| longest.max(target.target_ref.len()));
            for ally in &attack.policy.allies {
                longest = longest.max(ally.len());
            }
            longest
        }
    }
}

/// The strings one inventory endpoint carries. A player endpoint is a number and
/// carries none.
fn endpoint_text_bytes(value: &wire_inventories::InventoryEndpoint) -> usize {
    match value {
        wire_inventories::InventoryEndpoint::PlayerInventory(_) => 0,
        wire_inventories::InventoryEndpoint::Warehouse(handle)
        | wire_inventories::InventoryEndpoint::ResidentEquipment(handle)
        | wire_inventories::InventoryEndpoint::ResidentCarry(handle) => handle.len(),
    }
}

/// One contract resident kind as the server's own.
fn kind(value: wire::ResidentKind) -> ScriptResidentKind {
    match value {
        wire::ResidentKind::Villager => ScriptResidentKind::Villager,
    }
}

/// One contract work order as the server's own.
fn work_order(value: wire::WorkOrder) -> ScriptResidentWorkOrder {
    let area = |work: wire::AreaToolWork| (work.area, work.tool);
    match value {
        wire::WorkOrder::Harvest(work) => {
            let (area, tool) = area(work);
            ScriptResidentWorkOrder::Harvest {
                area: work_area(area),
                tool,
            }
        }
        wire::WorkOrder::CutTree(work) => {
            let (area, tool) = area(work);
            ScriptResidentWorkOrder::CutTree {
                area: work_area(area),
                tool,
            }
        }
        wire::WorkOrder::Mine(work) => {
            let (area, tool) = area(work);
            ScriptResidentWorkOrder::Mine {
                area: work_area(area),
                tool,
            }
        }
        wire::WorkOrder::Fish(work) => {
            let (area, tool) = area(work);
            ScriptResidentWorkOrder::Fish {
                area: work_area(area),
                tool,
            }
        }
        wire::WorkOrder::TendLivestock(work) => ScriptResidentWorkOrder::TendLivestock {
            area: work_area(work.area),
            feed: work.feed,
        },
        wire::WorkOrder::Haul(haul) => ScriptResidentWorkOrder::Haul {
            source: endpoint(haul.source),
            destination: endpoint(haul.destination),
            item: haul.item,
        },
        wire::WorkOrder::Craft(craft) => ScriptResidentWorkOrder::Craft {
            recipe: craft.recipe,
            count: craft.count,
            station: work_area(craft.station),
        },
        wire::WorkOrder::Construct(construct) => ScriptResidentWorkOrder::Construct {
            structure_id: construct.structure_id,
            stage: construct.stage,
            expected_revision: construct.expected_revision,
        },
    }
}

/// One contract squad order as the server's own.
fn order(value: wire::Order) -> ScriptResidentOrder {
    match value {
        wire::Order::Follow(follow) => ScriptResidentOrder::Follow {
            target_player: follow.target_player,
            formation: formation(follow.formation),
        },
        wire::Order::Move(move_order) => ScriptResidentOrder::Move {
            dimension: move_order.dimension,
            anchor: position(move_order.anchor),
            heading_degrees: move_order.heading_degrees,
            formation: formation(move_order.formation),
        },
        wire::Order::Hold(hold) => ScriptResidentOrder::Hold {
            anchor: position(hold.anchor),
            heading_degrees: hold.heading_degrees,
            formation: formation(hold.formation),
            engagement_radius: hold.engagement_radius,
        },
        wire::Order::Patrol(patrol) => ScriptResidentOrder::Patrol {
            waypoints: patrol.waypoints.into_iter().map(position).collect(),
            formation: formation(patrol.formation),
            engagement_radius: patrol.engagement_radius,
        },
        wire::Order::Garrison(garrison) => ScriptResidentOrder::Garrison {
            posts: garrison.posts,
            engagement_radius: garrison.engagement_radius,
        },
        wire::Order::Attack(attack) => ScriptResidentOrder::Attack {
            targets: attack
                .targets
                .into_iter()
                .map(|target| {
                    ScriptOrderTargetRef::new(
                        target.target_ref,
                        target.policy_revision,
                        target.expires_revision,
                    )
                })
                .collect(),
            policy: {
                let mut policy = ScriptEngagementPolicy::new(
                    attack.policy.revision,
                    attack.policy.allies,
                    attack
                        .policy
                        .permitted
                        .into_iter()
                        .map(hostile_category)
                        .collect(),
                );
                policy.officer = attack.policy.officer;
                policy.rally = Some(position(attack.policy.rally));
                policy
            },
        },
        wire::Order::Retreat(retreat) => ScriptResidentOrder::Retreat {
            anchor: position(retreat.anchor),
            formation: formation(retreat.formation),
        },
    }
}

/// One contract block cell as the server's own.
fn position(value: wire::BlockPosition) -> ScriptBlockPosition {
    ScriptBlockPosition::new(value.x, value.y, value.z)
}

/// One contract work area as the server's own.
fn work_area(value: wire::WorkArea) -> ScriptWorkArea {
    ScriptWorkArea::new(value.dimension, position(value.min), position(value.max))
}

/// One inventory endpoint of a haul as the server's own.
fn endpoint(value: wire_inventories::InventoryEndpoint) -> ScriptInventoryEndpoint {
    match value {
        wire_inventories::InventoryEndpoint::PlayerInventory(player_id) => {
            ScriptInventoryEndpoint::PlayerInventory { player_id }
        }
        wire_inventories::InventoryEndpoint::Warehouse(handle) => {
            ScriptInventoryEndpoint::Warehouse { handle }
        }
        wire_inventories::InventoryEndpoint::ResidentEquipment(handle) => {
            ScriptInventoryEndpoint::ResidentEquipment { handle }
        }
        wire_inventories::InventoryEndpoint::ResidentCarry(handle) => {
            ScriptInventoryEndpoint::ResidentCarry { handle }
        }
    }
}

/// One contract formation as the server's own.
fn formation(value: wire::Formation) -> ScriptFormation {
    ScriptFormation::new(formation_kind(value.kind), value.spacing)
}

/// One contract formation kind as the server's own.
fn formation_kind(value: wire::FormationKind) -> ScriptFormationKind {
    match value {
        wire::FormationKind::Line => ScriptFormationKind::Line,
        wire::FormationKind::Column => ScriptFormationKind::Column,
        wire::FormationKind::Wedge => ScriptFormationKind::Wedge,
        wire::FormationKind::Square => ScriptFormationKind::Square,
    }
}

/// One contract hostile category as the server's own.
fn hostile_category(value: wire::HostileCategory) -> ScriptHostileCategory {
    match value {
        wire::HostileCategory::Hostile => ScriptHostileCategory::Hostile,
        wire::HostileCategory::Player => ScriptHostileCategory::Player,
        wire::HostileCategory::OwnedResident => ScriptHostileCategory::OwnedResident,
        wire::HostileCategory::NeutralAnimal => ScriptHostileCategory::NeutralAnimal,
    }
}

/// One server resident snapshot as the contract names it, or nothing when a
/// member the contract does not name appears.
fn snapshot(value: &ScriptResidentSnapshot) -> Option<wire::ResidentSnapshot> {
    Some(wire::ResidentSnapshot {
        handle: value.handle.clone(),
        entity_uuid: value.entity_uuid.clone(),
        lifecycle: lifecycle(value.lifecycle)?,
        revision: value.revision,
        generation_id: value.generation_id.clone(),
        pois: wire::ResidentPois {
            home: value.pois.home.clone(),
            work: value.pois.work.clone(),
            meeting: value.pois.meeting.clone(),
        },
        loaded: value.loaded.as_ref().map(loaded_state),
    })
}

/// One server lifecycle as the contract names it.
fn lifecycle(value: ScriptResidentLifecycle) -> Option<wire::ResidentLifecycle> {
    Some(match value {
        ScriptResidentLifecycle::AliveLoaded => wire::ResidentLifecycle::AliveLoaded,
        ScriptResidentLifecycle::AliveUnloaded => wire::ResidentLifecycle::AliveUnloaded,
        ScriptResidentLifecycle::Dead => wire::ResidentLifecycle::Dead,
        ScriptResidentLifecycle::Released => wire::ResidentLifecycle::Released,
        _ => return None,
    })
}

/// One server loaded state as the contract names it. The carried summaries are a
/// bounded read of the live entity and carry no variant of their own.
fn loaded_state(value: &ScriptResidentLoadedState) -> wire::ResidentLoadedState {
    wire::ResidentLoadedState {
        x: value.x,
        y: value.y,
        z: value.z,
        health: value.health,
        carried: value
            .carried
            .iter()
            .map(
                |item: &ScriptResidentItemSummary| wire::ResidentItemSummary {
                    item_id: item.item_id.clone(),
                    count: item.count,
                },
            )
            .collect(),
    }
}

/// One server work assignment as the contract names it, or nothing when a state
/// or pause reason the contract does not name appears.
fn work_assignment(value: &ScriptWorkAssignment) -> Option<wire::WorkAssignment> {
    Some(wire::WorkAssignment {
        handle: value.handle.clone(),
        state: work_state(value.state)?,
        reason: match value.reason {
            Some(reason) => Some(pause_reason(reason)?),
            None => None,
        },
        work_units_done: value.work_units_done,
        work_units_planned: value.work_units_planned,
        changes: value.changes.iter().map(item_change).collect(),
        revision: value.revision,
    })
}

/// One server work state as the contract names it.
fn work_state(value: ScriptWorkState) -> Option<wire::WorkState> {
    Some(match value {
        ScriptWorkState::Accepted => wire::WorkState::Accepted,
        ScriptWorkState::Running => wire::WorkState::Running,
        ScriptWorkState::Paused => wire::WorkState::Paused,
        ScriptWorkState::Committed => wire::WorkState::Committed,
        ScriptWorkState::Cancelled => wire::WorkState::Cancelled,
        _ => return None,
    })
}

/// One server pause reason as the contract names it.
fn pause_reason(value: ScriptWorkPauseReason) -> Option<wire::WorkPauseReason> {
    Some(match value {
        ScriptWorkPauseReason::Unloaded => wire::WorkPauseReason::Unloaded,
        ScriptWorkPauseReason::NoWorkers => wire::WorkPauseReason::NoWorkers,
        ScriptWorkPauseReason::MissingInput => wire::WorkPauseReason::MissingInput,
        ScriptWorkPauseReason::MissingTool => wire::WorkPauseReason::MissingTool,
        ScriptWorkPauseReason::MissingStation => wire::WorkPauseReason::MissingStation,
        ScriptWorkPauseReason::BlockedRoute => wire::WorkPauseReason::BlockedRoute,
        ScriptWorkPauseReason::Interrupted => wire::WorkPauseReason::Interrupted,
        ScriptWorkPauseReason::Protected => wire::WorkPauseReason::Protected,
        ScriptWorkPauseReason::Unsupported => wire::WorkPauseReason::Unsupported,
        ScriptWorkPauseReason::NoStorage => wire::WorkPauseReason::NoStorage,
        _ => return None,
    })
}

/// One server item change as the contract names it.
fn item_change(value: &ScriptItemChange) -> wire::ItemChange {
    wire::ItemChange {
        item_id: value.item_id.clone(),
        delta: value.delta,
    }
}

/// One server member outcome as the contract names it, or nothing when a state
/// the contract does not name appears.
fn member_outcome(value: &ScriptOrderMemberOutcome) -> Option<wire::OrderMemberOutcome> {
    Some(wire::OrderMemberOutcome {
        handle: value.handle.clone(),
        state: member_state(value.state)?,
        formation_slot: value.formation_slot,
        targets: value
            .targets
            .iter()
            .map(order_target)
            .collect::<Option<Vec<_>>>()?,
    })
}

/// One server member state as the contract names it.
fn member_state(value: ScriptOrderMemberState) -> Option<wire::OrderMemberState> {
    Some(match value {
        ScriptOrderMemberState::Applied => wire::OrderMemberState::Applied,
        ScriptOrderMemberState::BlockedRoute => wire::OrderMemberState::BlockedRoute,
        ScriptOrderMemberState::Unloaded => wire::OrderMemberState::Unloaded,
        ScriptOrderMemberState::Dead => wire::OrderMemberState::Dead,
        ScriptOrderMemberState::Migrating => wire::OrderMemberState::Migrating,
        ScriptOrderMemberState::Forbidden => wire::OrderMemberState::Forbidden,
        ScriptOrderMemberState::StaleRevision => wire::OrderMemberState::StaleRevision,
        _ => return None,
    })
}

/// One server order target as the contract names it.
fn order_target(value: &ScriptOrderTarget) -> Option<wire::OrderTarget> {
    Some(wire::OrderTarget {
        target_ref: value.target_ref.clone(),
        policy_revision: value.policy_revision,
        expires_revision: value.expires_revision,
        category: contract_hostile_category(value.category)?,
        position: wire::BlockPosition {
            x: value.position.x,
            y: value.position.y,
            z: value.position.z,
        },
    })
}

/// One server hostile category as the contract names it.
fn contract_hostile_category(value: ScriptHostileCategory) -> Option<wire::HostileCategory> {
    Some(match value {
        ScriptHostileCategory::Hostile => wire::HostileCategory::Hostile,
        ScriptHostileCategory::Player => wire::HostileCategory::Player,
        ScriptHostileCategory::OwnedResident => wire::HostileCategory::OwnedResident,
        ScriptHostileCategory::NeutralAnimal => wire::HostileCategory::NeutralAnimal,
        _ => return None,
    })
}

/// One server combat event as the contract names it. A committed combat outcome
/// carries no closed variant of its own beyond the category above.
fn combat_event(value: &ScriptCombatEvent) -> wire::CombatEvent {
    wire::CombatEvent {
        event_id: value.event_id,
        revision: value.revision,
        attacker_handle: value.attacker_handle.clone(),
        victim_target_ref: value.victim_target_ref.clone(),
        order_revision: value.order_revision,
        damage_milli: value.damage_milli,
        killed: value.killed,
    }
}

/// One server demobilisation result as the contract names it, or nothing when a
/// state or reason the contract does not name appears.
fn demobilize_result(value: &ScriptDemobilizeResult) -> Option<wire::DemobilizeResult> {
    Some(wire::DemobilizeResult {
        handle: value.handle.clone(),
        state: match value.state {
            ScriptDemobilizeState::Demobilizing => wire::DemobilizeState::Demobilizing,
            ScriptDemobilizeState::Civilian => wire::DemobilizeState::Civilian,
            _ => return None,
        },
        reason: match value.reason {
            Some(reason) => Some(pause_reason(reason)?),
            None => None,
        },
        returned: value.returned.iter().map(item_change).collect(),
        revision: value.revision,
    })
}
