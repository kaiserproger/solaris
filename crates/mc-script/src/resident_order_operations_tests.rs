//! DTO-level acceptance tests for resident work orders and squad orders (C4).

use crate::{
    MAX_RESIDENT_ORDER_HANDLES, MAX_RESIDENT_SQUAD_MEMBERS, ScriptBlockPosition,
    ScriptEngagementPolicy, ScriptFormation, ScriptFormationKind, ScriptHostileCategory,
    ScriptInventoryEndpoint, ScriptOperation, ScriptOperationOutcome, ScriptOperationPayload,
    ScriptOperationRequest, ScriptOrderTargetRef, ScriptResidentOrder,
    ScriptResidentOrderOperation, ScriptResidentOrderResult, ScriptResidentWorkOrder,
    ScriptWorkArea, warehouse_handle,
};

fn area(extent: i32) -> ScriptWorkArea {
    ScriptWorkArea::new(
        "minecraft:overworld".to_owned(),
        ScriptBlockPosition::new(0, 64, 0),
        ScriptBlockPosition::new(extent, 64, 0),
    )
}

fn formation() -> ScriptFormation {
    ScriptFormation::new(ScriptFormationKind::Line, 4)
}

fn issue(order: ScriptResidentOrder) -> Result<ScriptOperationRequest, crate::ScriptDtoError> {
    ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::IssueOrder {
                operation_id: "order-1".to_owned(),
                handles: vec!["a".repeat(32), "b".repeat(32)],
                expected_order_revisions: vec![0, 0],
                order,
            },
        },
    )
}

#[test]
fn work_orders_require_a_concrete_bounded_target() {
    // A harvest whose plot exceeds the bounded work axis is refused.
    let oversized = ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::AssignWork {
                operation_id: "work-1".to_owned(),
                handle: "a".repeat(32),
                work: ScriptResidentWorkOrder::Harvest {
                    area: area(64),
                    tool: "minecraft:iron_hoe".to_owned(),
                },
                work_units: 16,
                expected_revision: 0,
            },
        },
    );
    assert!(oversized.is_err());
    // A zero-count craft, a non-single-cell station, and a zero work budget are refused.
    let invalid: [ScriptResidentWorkOrder; 3] = [
        ScriptResidentWorkOrder::Craft {
            recipe: "minecraft:stick".to_owned(),
            count: 0,
            station: area(0),
        },
        ScriptResidentWorkOrder::Craft {
            recipe: "minecraft:stick".to_owned(),
            count: 1,
            station: area(1),
        },
        ScriptResidentWorkOrder::Mine {
            area: area(4),
            tool: "minecraft:iron_pickaxe".to_owned(),
        },
    ];
    for work in invalid {
        let request = ScriptOperationRequest::try_new(
            "request",
            ScriptOperation::ResidentOrder {
                operation: ScriptResidentOrderOperation::AssignWork {
                    operation_id: "work-2".to_owned(),
                    handle: "a".repeat(32),
                    work: work.clone(),
                    work_units: if matches!(work, ScriptResidentWorkOrder::Craft { .. }) {
                        16
                    } else {
                        0
                    },
                    expected_revision: 0,
                },
            },
        );
        assert!(request.is_err(), "invalid work order {work:?} was accepted");
    }
}

#[test]
fn a_haul_names_either_endpoint_and_one_real_item() {
    // A haul is a move between two distinct endpoints, and the item it takes is
    // a resource id - the same contract every other named resource obeys.
    let endpoints = (
        ScriptInventoryEndpoint::ResidentCarry {
            handle: "a".repeat(32),
        },
        ScriptInventoryEndpoint::Warehouse {
            handle: warehouse_handle("plugin", &"b".repeat(32), 0).expect("warehouse handle"),
        },
    );
    let haul = |source: ScriptInventoryEndpoint,
                destination: ScriptInventoryEndpoint,
                item: Option<String>| {
        ScriptOperationRequest::try_new(
            "request",
            ScriptOperation::ResidentOrder {
                operation: ScriptResidentOrderOperation::AssignWork {
                    operation_id: "work-3".to_owned(),
                    handle: "a".repeat(32),
                    work: ScriptResidentWorkOrder::Haul {
                        source,
                        destination,
                        item,
                    },
                    work_units: 4,
                    expected_revision: 0,
                },
            },
        )
    };
    // Either direction is a real order, with or without a named item.
    assert!(
        haul(endpoints.0.clone(), endpoints.1.clone(), None).is_ok(),
        "a worker's haul into a bound container is a move core can plan"
    );
    assert!(
        haul(
            endpoints.1.clone(),
            endpoints.0.clone(),
            Some("minecraft:iron_hoe".to_owned())
        )
        .is_ok(),
        "a withdrawal of one named item is a move core can plan"
    );
    // The same endpoint on both sides is not a move, and a name that is not a
    // resource id is not an item.
    assert!(haul(endpoints.0.clone(), endpoints.0.clone(), None).is_err());
    for item in ["Iron_Hoe", "minecraft:", "minecraft:iron hoe", ""] {
        assert!(
            haul(
                endpoints.1.clone(),
                endpoints.0.clone(),
                Some(item.to_owned())
            )
            .is_err(),
            "item {item:?} was accepted"
        );
    }
}

#[test]
fn patrol_and_attack_orders_are_bounded_and_closed() {
    let too_few = issue(ScriptResidentOrder::Patrol {
        waypoints: vec![ScriptBlockPosition::new(0, 64, 0)],
        formation: formation(),
        engagement_radius: 8,
    });
    assert!(too_few.is_err());
    let too_many = issue(ScriptResidentOrder::Patrol {
        waypoints: (0..32)
            .map(|index| ScriptBlockPosition::new(index * 2, 64, 0))
            .collect(),
        formation: formation(),
        engagement_radius: 8,
    });
    assert!(too_many.is_err());
    let empty_attack = issue(ScriptResidentOrder::Attack {
        targets: Vec::new(),
        policy: ScriptEngagementPolicy::new(1, Vec::new(), vec![ScriptHostileCategory::Hostile]),
    });
    assert!(empty_attack.is_err());
    // Player targets are only hostile when the policy explicitly permits them.
    let policy = ScriptEngagementPolicy::new(1, Vec::new(), vec![ScriptHostileCategory::Hostile]);
    let request = issue(ScriptResidentOrder::Attack {
        targets: vec![ScriptOrderTargetRef::new("t1".to_owned(), 1, 64)],
        policy,
    })
    .expect("valid attack order");
    let ScriptOperation::ResidentOrder { operation } = request.operation() else {
        panic!("expected a resident order operation");
    };
    let ScriptResidentOrderOperation::IssueOrder { order, .. } = operation else {
        panic!("expected an issue order");
    };
    let ScriptResidentOrder::Attack { policy, .. } = order else {
        panic!("expected an attack order");
    };
    assert!(!policy.permitted.contains(&ScriptHostileCategory::Player));
    assert!(policy.permitted.contains(&ScriptHostileCategory::Hostile));
}

#[test]
fn order_handles_and_revisions_are_canonicalized_for_idempotency() {
    let request = ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::IssueOrder {
                operation_id: "order-2".to_owned(),
                handles: vec!["b".repeat(32), "a".repeat(32)],
                expected_order_revisions: vec![5, 3],
                order: ScriptResidentOrder::Move {
                    dimension: "minecraft:overworld".to_owned(),
                    anchor: ScriptBlockPosition::new(4, 64, 4),
                    heading_degrees: 90,
                    formation: formation(),
                },
            },
        },
    )
    .expect("valid move order");
    let ScriptOperation::ResidentOrder { operation } = request.operation() else {
        panic!("expected a resident order operation");
    };
    let ScriptResidentOrderOperation::IssueOrder {
        handles,
        expected_order_revisions,
        ..
    } = operation
    else {
        panic!("expected an issue order");
    };
    assert_eq!(handles[0], "a".repeat(32));
    assert_eq!(expected_order_revisions, &[3, 5]);
    assert_eq!(request.operation_id(), Some("order-2"));
}

#[test]
fn squad_handles_are_bounded_to_the_core_limit() {
    let handles = (0..=MAX_RESIDENT_ORDER_HANDLES)
        .map(|index| format!("{index:0>32}"))
        .collect::<Vec<_>>();
    let request = ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::CancelOrder {
                operation_id: "cancel-1".to_owned(),
                expected_order_revisions: vec![0; handles.len()],
                handles,
            },
        },
    );
    assert!(request.is_err());
    // A gameplay squad of exactly the squad bound is accepted; one more member
    // than the core handle bound is refused above.
    let squad = (0..MAX_RESIDENT_SQUAD_MEMBERS)
        .map(|index| format!("{index:0>32}"))
        .collect::<Vec<_>>();
    let request = ScriptOperationRequest::try_new(
        "request",
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::CancelOrder {
                operation_id: "cancel-squad".to_owned(),
                expected_order_revisions: vec![0; squad.len()],
                handles: squad,
            },
        },
    );
    assert!(request.is_ok());
}

#[test]
fn order_results_carry_member_reasons_and_committed_combat() {
    let outcome = ScriptOperationOutcome::rejected_with_payload(
        crate::ScriptOperationFailure::Forbidden,
        ScriptOperationPayload::ResidentOrder {
            result: Box::new(ScriptResidentOrderResult::Order {
                order_revision: 0,
                members: vec![crate::ScriptOrderMemberOutcome::new(
                    "a".repeat(32),
                    crate::ScriptOrderMemberState::Forbidden,
                    None,
                    Vec::new(),
                )],
                combat: Vec::new(),
            }),
        },
    );
    assert_eq!(
        outcome.failure(),
        Some(crate::ScriptOperationFailure::Forbidden)
    );
    assert!(outcome.validate().is_ok());
}

#[test]
fn resident_inventory_endpoints_validate_their_slots() {
    let equipment = ScriptInventoryEndpoint::ResidentEquipment {
        handle: "a".repeat(32),
    };
    assert!(equipment.validate().is_ok());
    assert_eq!(equipment.resident_handle(), Some("a".repeat(32).as_str()));
    let carry = ScriptInventoryEndpoint::ResidentCarry {
        handle: "a".repeat(32),
    };
    assert_eq!(
        carry,
        ScriptInventoryEndpoint::ResidentCarry {
            handle: "a".repeat(32)
        }
    );
}
