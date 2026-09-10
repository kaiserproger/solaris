use serde_json::{Value, json};

use mc_script::ScriptOperationRequest;

fn transfer_request() -> Value {
    let player = json!({"kind": "player_inventory", "player_id": 7});
    let warehouse = json!({"kind": "warehouse", "handle": "completed-container"});
    let fence = json!({"revision": 3, "snapshot_hash": "a".repeat(64)});
    json!({
        "request_id": "transfer-request",
        "operation": {
            "kind": "inventory",
            "operation": {
                "kind": "transfer",
                "operation_id": "transfer-operation",
                "actor_id": 7,
                "transfers": [{
                    "source": player,
                    "source_slot": 9,
                    "destination": warehouse,
                    "destination_slot": 0,
                    "count": 2
                }],
                "expected_revisions": [
                    {"endpoint": player, "fence": fence},
                    {"endpoint": warehouse, "fence": fence}
                ]
            }
        }
    })
}

#[test]
fn owned_transfer_wire_rejects_missing_or_duplicate_participant_fences() {
    let mut missing = transfer_request();
    missing["operation"]["operation"]["expected_revisions"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(serde_json::from_value::<ScriptOperationRequest>(missing).is_err());

    let mut duplicate = transfer_request();
    duplicate["operation"]["operation"]["expected_revisions"][1] =
        duplicate["operation"]["operation"]["expected_revisions"][0].clone();
    assert!(serde_json::from_value::<ScriptOperationRequest>(duplicate).is_err());
}

#[test]
fn owned_transfer_wire_validates_source_and_forbids_coordinate_warehouse_access() {
    let mut invalid_source = transfer_request();
    invalid_source["operation"]["operation"]["transfers"][0]["source"]["player_id"] = json!(0);
    invalid_source["operation"]["operation"]["expected_revisions"][0]["endpoint"]["player_id"] =
        json!(0);
    assert!(serde_json::from_value::<ScriptOperationRequest>(invalid_source).is_err());

    let mut coordinates = transfer_request();
    coordinates["operation"]["operation"]["transfers"][0]["destination"]["position"] =
        json!([0, 64, 0]);
    assert!(serde_json::from_value::<ScriptOperationRequest>(coordinates).is_err());
}

#[test]
fn owned_transfer_fence_order_does_not_change_operation_identity() {
    let first: ScriptOperationRequest = serde_json::from_value(transfer_request()).unwrap();
    let mut reordered = transfer_request();
    reordered["operation"]["operation"]["expected_revisions"]
        .as_array_mut()
        .unwrap()
        .reverse();
    let reordered: ScriptOperationRequest = serde_json::from_value(reordered).unwrap();
    assert_eq!(first.operation(), reordered.operation());
}
