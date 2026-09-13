use crate::{
    MAX_OWNED_INVENTORY_ITEM_BUDGET, ScriptDtoError, ScriptInventoryEndpoint, ScriptInventoryFence,
    ScriptOperation, ScriptOperationRequest, ScriptOwnedInventoryOperation,
    ScriptOwnedItemTransfer,
};

fn player() -> ScriptInventoryEndpoint {
    ScriptInventoryEndpoint::PlayerInventory { player_id: 7 }
}

fn fence() -> ScriptInventoryFence {
    ScriptInventoryFence::try_new(3, "a".repeat(64)).unwrap()
}

fn transfer(counts: &[u32]) -> Result<ScriptOperationRequest, ScriptDtoError> {
    let endpoint = player();
    let transfers = counts
        .iter()
        .enumerate()
        .map(|(index, count)| {
            ScriptOwnedItemTransfer::new(
                endpoint.clone(),
                9,
                endpoint.clone(),
                u8::try_from(10 + index).unwrap(),
                *count,
            )
        })
        .collect();
    ScriptOperationRequest::try_new(
        "transfer-request",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Transfer {
                operation_id: "transfer-operation".to_owned(),
                actor_id: 7,
                transfers,
                expected_revisions: vec![crate::ScriptInventoryExpectedRevision::new(
                    endpoint,
                    fence(),
                )],
            },
        },
    )
}

#[test]
fn owned_transfer_enforces_one_bounded_item_budget_per_request() {
    assert!(transfer(&[2048, 2048]).is_ok());
    assert!(matches!(
        transfer(&[2048, 2049]),
        Err(ScriptDtoError::InvalidBounds)
    ));
    assert!(transfer(&[MAX_OWNED_INVENTORY_ITEM_BUDGET]).is_ok());
    assert!(transfer(&[MAX_OWNED_INVENTORY_ITEM_BUDGET, 1]).is_err());
}
