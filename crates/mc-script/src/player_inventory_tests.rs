use std::num::NonZeroUsize;

use super::*;

fn deltas() -> Vec<ScriptInventoryResourceDelta> {
    vec![
        ScriptInventoryResourceDelta::try_new("minecraft:emerald", -3).unwrap(),
        ScriptInventoryResourceDelta::try_new("minecraft:apple", 2).unwrap(),
    ]
}

#[tokio::test]
async fn admitted_player_inventory_transaction_is_capability_gated_and_targeted() {
    let transaction =
        ScriptPlayerInventoryTransaction::try_new("kit-starter", ScriptPlayerId::new(7), deltas())
            .unwrap();
    assert_eq!(transaction.request_id(), "kit-starter");
    assert_eq!(transaction.player_id(), ScriptPlayerId::new(7));
    assert_eq!(transaction.deltas(), deltas());

    let command = ScriptCommand::PlayerInventoryTransaction {
        transaction: transaction.clone(),
    };
    assert_eq!(
        command.required_capability_kind(),
        Some(ScriptCommandCapabilityKind::PlayerInventory)
    );
    let mut denied = CommandBatch::new(NonZeroUsize::new(1).unwrap());
    assert_eq!(
        denied.try_push_authorized(command.clone(), &CommandCapabilities::default()),
        Err(CommandBatchError::PermissionDenied {
            capability: ScriptCommandCapabilityKind::PlayerInventory,
        })
    );
    assert!(denied.commands().is_empty());

    let manifest = ScriptPluginManifest::new("kits", "Kits", "0.1.0", SCRIPT_API_VERSION)
        .declare_player_inventory()
        .validate()
        .unwrap();
    let admission = HostCommandAdmission::from_manifest(&manifest);
    let (boundary, endpoint) =
        script_boundary_pair(NonZeroUsize::new(1).unwrap(), NonZeroUsize::new(1).unwrap());
    let mut batch = CommandBatch::new(NonZeroUsize::new(1).unwrap());
    batch
        .try_push_authorized(command, &manifest.to_command_capabilities())
        .unwrap();
    endpoint.try_submit_plugin_batch(&admission, batch).unwrap();

    let admitted = boundary
        .accept_host_command(boundary.recv_command().await.unwrap())
        .unwrap();
    let result = admitted.player_inventory_transaction_result(None).unwrap();
    assert_eq!(result.target_plugin_id(), Some("kits"));
    assert_eq!(result.event_name(), "player.inventory_transaction_result");
    assert!(matches!(
        result.kind(),
        ScriptEventKind::PlayerInventoryTransactionResult {
            request_id,
            player_id,
            failure: None,
        } if request_id == "kit-starter" && *player_id == ScriptPlayerId::new(7)
    ));

    for (failure, code) in [
        (
            ScriptPlayerInventoryFailure::PlayerUnavailable,
            "player_unavailable",
        ),
        (
            ScriptPlayerInventoryFailure::UnknownResource,
            "unknown_resource",
        ),
        (
            ScriptPlayerInventoryFailure::InsufficientResource,
            "insufficient_resource",
        ),
        (
            ScriptPlayerInventoryFailure::InventoryFull,
            "inventory_full",
        ),
        (
            ScriptPlayerInventoryFailure::RuntimeUnavailable,
            "runtime_unavailable",
        ),
    ] {
        assert_eq!(failure.as_str(), code);
    }
}

#[test]
fn player_inventory_transaction_rejects_empty_duplicate_and_oversized_inputs() {
    assert!(matches!(
        ScriptPlayerInventoryTransaction::try_new("empty", ScriptPlayerId::new(1), Vec::new()),
        Err(ScriptDtoError::EmptyTransaction)
    ));
    let duplicate = ScriptInventoryResourceDelta::try_new("minecraft:apple", 1).unwrap();
    assert!(matches!(
        ScriptPlayerInventoryTransaction::try_new(
            "duplicate",
            ScriptPlayerId::new(1),
            vec![duplicate.clone(), duplicate],
        ),
        Err(ScriptDtoError::DuplicateId {
            field: "inventory resource id",
            ..
        })
    ));
    let too_many = (0..=MAX_INVENTORY_STORAGE_MUTATIONS)
        .map(|index| {
            ScriptInventoryResourceDelta::try_new(format!("minecraft:item_{index}"), 1).unwrap()
        })
        .collect();
    assert!(matches!(
        ScriptPlayerInventoryTransaction::try_new("too-many", ScriptPlayerId::new(1), too_many,),
        Err(ScriptDtoError::TooManyEntries {
            field: "player inventory transaction",
            max: MAX_INVENTORY_STORAGE_MUTATIONS,
        })
    ));
}
