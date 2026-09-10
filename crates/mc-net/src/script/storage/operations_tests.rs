use mc_script::{
    ScriptOperation, ScriptOperationFailure, ScriptOperationOutcome, ScriptOperationPayload,
    ScriptOperationRequest, ScriptStorageMutation,
};

use super::{PluginStorage, operations::OperationExecution};

fn execute(
    storage: &mut PluginStorage,
    owner: &str,
    request_id: &str,
    operation: ScriptOperation,
) -> ScriptOperationOutcome {
    let request = ScriptOperationRequest::try_new(request_id, operation).unwrap();
    match storage.execute_operation(owner, &request).unwrap() {
        OperationExecution::Reply(outcome) => outcome,
        OperationExecution::Durable(receipt) => receipt.outcome,
    }
}

fn batch(value: &str) -> ScriptOperation {
    ScriptOperation::StorageBatch {
        operation_id: "purchase-1".to_owned(),
        mutations: vec![
            ScriptStorageMutation::compare_and_swap("ledger", None, value).unwrap(),
            ScriptStorageMutation::compare_and_swap("intent", None, "paid").unwrap(),
        ],
    }
}

#[test]
fn operation_identity_replays_after_acknowledgement_compaction_and_restart() {
    let world = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(world.path()).unwrap();
    let request = ScriptOperationRequest::try_new("initial", batch("10")).unwrap();
    let OperationExecution::Durable(receipt) = storage.execute_operation("shop", &request).unwrap()
    else {
        panic!("purchase was rejected");
    };
    storage.acknowledge_operation(&receipt).unwrap();
    storage.compact().unwrap();
    drop(storage);

    let mut storage = PluginStorage::open(world.path()).unwrap();
    let replay = execute(&mut storage, "shop", "retry", batch("10"));
    assert_eq!(replay, receipt.outcome);
    assert_eq!(storage.get("shop", "ledger"), Some(("10".to_owned(), 1)));
    assert_eq!(storage.get("shop", "intent"), Some(("paid".to_owned(), 1)));
    let conflict = execute(&mut storage, "shop", "changed", batch("20"));
    assert_eq!(
        conflict.failure(),
        Some(ScriptOperationFailure::OperationConflict)
    );
    assert_eq!(storage.get("shop", "ledger"), Some(("10".to_owned(), 1)));
    let foreign = execute(
        &mut storage,
        "other",
        "query",
        ScriptOperation::Status {
            operation_id: "purchase-1".to_owned(),
        },
    );
    assert_eq!(foreign.failure(), Some(ScriptOperationFailure::NotFound));
    let separate = execute(&mut storage, "other", "purchase", batch("20"));
    assert_eq!(separate.revision(), Some(2));
    assert_eq!(storage.get("other", "ledger"), Some(("20".to_owned(), 2)));
}

#[test]
fn scan_cursor_keeps_original_values_and_is_owner_scoped_and_replayable() {
    let world = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(world.path()).unwrap();
    for key in ["item.a", "item.b", "item.c", "item.d"] {
        storage.compare_and_swap("shop", key, None, key).unwrap();
    }
    storage
        .compare_and_swap("other", "item.secret", None, "private")
        .unwrap();
    let scan = |cursor| ScriptOperation::StorageScan {
        prefix: "item.".to_owned(),
        cursor,
        limit: 2,
    };
    let first = execute(&mut storage, "shop", "first", scan(None));
    let ScriptOperationPayload::StoragePage {
        entries,
        cursor: Some(cursor),
    } = first.payload()
    else {
        panic!("first page must have a continuation");
    };
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.key.as_str())
            .collect::<Vec<_>>(),
        ["item.a", "item.b"]
    );
    let cursor = cursor.clone();
    storage
        .compare_and_swap("shop", "item.c", Some(3), "changed")
        .unwrap();
    storage.delete("shop", "item.d", Some(4)).unwrap();
    storage
        .compare_and_swap("shop", "item.e", None, "new")
        .unwrap();
    let denied = execute(&mut storage, "other", "foreign", scan(Some(cursor.clone())));
    assert_eq!(denied.failure(), Some(ScriptOperationFailure::Forbidden));
    let second = execute(&mut storage, "shop", "second", scan(Some(cursor.clone())));
    assert_eq!(second.revision(), first.revision());
    let ScriptOperationPayload::StoragePage {
        entries,
        cursor: None,
    } = second.payload()
    else {
        panic!("second page must finish the snapshot");
    };
    assert_eq!(
        entries
            .iter()
            .map(|entry| (entry.key.as_str(), entry.value.as_str()))
            .collect::<Vec<_>>(),
        [("item.c", "item.c"), ("item.d", "item.d")]
    );
    assert_eq!(
        execute(&mut storage, "shop", "retry", scan(Some(cursor.clone()))),
        second
    );
    drop(storage);
    let mut storage = PluginStorage::open(world.path()).unwrap();
    let expired = execute(&mut storage, "shop", "restart", scan(Some(cursor)));
    assert_eq!(
        expired.failure(),
        Some(ScriptOperationFailure::CursorExpired)
    );
}

#[test]
fn unknown_sync_keeps_a_recoverable_operation_not_a_rejected_purchase() {
    let world = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(world.path()).unwrap();
    storage.inject_fault_for_test(super::StorageFaultPoint::Sync);
    let request = ScriptOperationRequest::try_new("initial", batch("10")).unwrap();
    assert!(matches!(
        storage.execute_operation("shop", &request),
        Err(super::PluginStorageMutationError::DurabilityUnknown(_))
    ));
    drop(storage);
    let mut storage = PluginStorage::open(world.path()).unwrap();
    let status = execute(
        &mut storage,
        "shop",
        "recovery",
        ScriptOperation::Status {
            operation_id: "purchase-1".to_owned(),
        },
    );
    assert_eq!(status.revision(), Some(1));
    assert_eq!(status.failure(), None);
    assert_eq!(execute(&mut storage, "shop", "retry", batch("10")), status);
    assert_eq!(storage.get("shop", "ledger"), Some(("10".to_owned(), 1)));
}
