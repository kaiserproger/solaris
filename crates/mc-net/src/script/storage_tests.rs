use mc_script::{LuaHostConfig, ScriptCommand, ScriptEvent, ScriptStorageMutation, start_lua_host};

use super::storage::{PluginStorage, StorageFaultPoint, run_storage_actor_for_test};
use super::{ScriptRouter, ScriptRouterExit};
use crate::server::{
    CommandPermissionConfig, ScriptEventSink, ServerConfig, ShutdownHandle, bind_with_scripts,
};

fn journal_path(root: &std::path::Path) -> std::path::PathBuf {
    root.join("solaris/plugin-storage-v1/journal-v1.bin")
}

#[test]
fn storage_restarts_with_get_cas_and_delete_state() {
    let temp = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(temp.path()).unwrap();

    assert_eq!(storage.get("shop", "balance"), None);
    assert_eq!(
        storage
            .compare_and_swap("shop", "balance", None, "1")
            .unwrap(),
        Some(1)
    );
    assert_eq!(storage.get("shop", "balance"), Some(("1".to_owned(), 1)));
    assert_eq!(storage.delete("shop", "balance", Some(1)).unwrap(), Some(2));
    drop(storage);

    let storage = PluginStorage::open(temp.path()).unwrap();
    assert_eq!(storage.get("shop", "balance"), None);
}

#[test]
fn storage_rejects_absent_stale_and_cross_plugin_mutations() {
    let temp = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(temp.path()).unwrap();

    assert_eq!(
        storage
            .compare_and_swap("shop", "balance", Some(1), "1")
            .unwrap(),
        None
    );
    let version = storage
        .compare_and_swap("shop", "balance", None, "1")
        .unwrap()
        .unwrap();
    assert_eq!(
        storage
            .compare_and_swap("shop", "balance", Some(version + 1), "2")
            .unwrap(),
        None
    );
    assert_eq!(storage.delete("shop", "balance", None).unwrap(), None);
    assert_eq!(storage.get("other", "balance"), None);
    assert_eq!(
        storage
            .compare_and_swap("other", "balance", None, "2")
            .unwrap(),
        Some(version + 1)
    );
    assert_eq!(
        storage.get("shop", "balance"),
        Some(("1".to_owned(), version))
    );
}

#[test]
fn storage_batch_commits_every_key_at_one_revision_and_restarts() {
    let temp = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(temp.path()).unwrap();
    let first = storage
        .compare_and_swap("shop", "first", None, "old-first")
        .unwrap()
        .unwrap();
    let second = storage
        .compare_and_swap("shop", "second", None, "old-second")
        .unwrap()
        .unwrap();
    let mutations = vec![
        ScriptStorageMutation::compare_and_swap("first", Some(first), "new-first").unwrap(),
        ScriptStorageMutation::delete("second", Some(second)).unwrap(),
        ScriptStorageMutation::compare_and_swap("third", None, "new-third").unwrap(),
    ];

    assert!(storage.storage_batch_for_test("shop", &mutations).unwrap());
    assert_eq!(
        storage.get("shop", "first"),
        Some(("new-first".to_owned(), 3))
    );
    assert_eq!(storage.get("shop", "second"), None);
    assert_eq!(
        storage.get("shop", "third"),
        Some(("new-third".to_owned(), 3))
    );
    drop(storage);

    let storage = PluginStorage::open(temp.path()).unwrap();
    assert_eq!(
        storage.get("shop", "first"),
        Some(("new-first".to_owned(), 3))
    );
    assert_eq!(storage.get("shop", "second"), None);
    assert_eq!(
        storage.get("shop", "third"),
        Some(("new-third".to_owned(), 3))
    );
}

#[test]
fn storage_batch_rejects_stale_or_over_quota_without_partial_mutation() {
    let absent_delete = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(absent_delete.path()).unwrap();
    let mutations = vec![
        ScriptStorageMutation::compare_and_swap("would-create", None, "new").unwrap(),
        ScriptStorageMutation::delete("absent", None).unwrap(),
    ];
    assert!(!storage.storage_batch_for_test("shop", &mutations).unwrap());
    assert_eq!(storage.get("shop", "would-create"), None);

    let stale = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(stale.path()).unwrap();
    let first = storage
        .compare_and_swap("shop", "first", None, "old-first")
        .unwrap()
        .unwrap();
    let second = storage
        .compare_and_swap("shop", "second", None, "old-second")
        .unwrap()
        .unwrap();
    let mutations = vec![
        ScriptStorageMutation::compare_and_swap("first", Some(first), "new-first").unwrap(),
        ScriptStorageMutation::delete("second", Some(second + 1)).unwrap(),
    ];
    assert!(!storage.storage_batch_for_test("shop", &mutations).unwrap());
    assert_eq!(
        storage.get("shop", "first"),
        Some(("old-first".to_owned(), first))
    );
    assert_eq!(
        storage.get("shop", "second"),
        Some(("old-second".to_owned(), second))
    );

    let quota = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(quota.path()).unwrap();
    storage.fill_plugin_record_quota_for_test("shop");
    let mutations = vec![
        ScriptStorageMutation::compare_and_swap("existing-0", Some(1), "changed").unwrap(),
        ScriptStorageMutation::compare_and_swap("one-too-many", None, "new").unwrap(),
    ];
    assert!(storage.storage_batch_for_test("shop", &mutations).is_err());
    assert_eq!(storage.get("shop", "existing-0"), Some(("x".to_owned(), 1)));
    assert_eq!(storage.get("shop", "one-too-many"), None);
}

#[test]
fn storage_batch_write_failure_keeps_memory_and_disk_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(temp.path()).unwrap();
    let version = storage
        .compare_and_swap("shop", "balance", None, "old")
        .unwrap()
        .unwrap();
    storage.inject_fault_for_test(StorageFaultPoint::Write);
    let mutations =
        vec![ScriptStorageMutation::compare_and_swap("balance", Some(version), "new").unwrap()];

    assert!(storage.storage_batch_for_test("shop", &mutations).is_err());
    assert_eq!(
        storage.get("shop", "balance"),
        Some(("old".to_owned(), version))
    );
    drop(storage);

    let storage = PluginStorage::open(temp.path()).unwrap();
    assert_eq!(
        storage.get("shop", "balance"),
        Some(("old".to_owned(), version))
    );
}

#[test]
fn storage_batch_sync_unknown_replays_only_the_complete_crc_frame() {
    let temp = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(temp.path()).unwrap();
    let version = storage
        .compare_and_swap("shop", "balance", None, "old")
        .unwrap()
        .unwrap();
    storage.inject_fault_for_test(StorageFaultPoint::Sync);
    let mutations =
        vec![ScriptStorageMutation::compare_and_swap("balance", Some(version), "new").unwrap()];

    assert!(storage.storage_batch_for_test("shop", &mutations).is_err());
    assert_eq!(
        storage.get("shop", "balance"),
        Some(("old".to_owned(), version))
    );
    drop(storage);

    let storage = PluginStorage::open(temp.path()).unwrap();
    assert_eq!(
        storage.get("shop", "balance"),
        Some(("new".to_owned(), version + 1))
    );
}

#[test]
fn durable_request_identity_replays_without_mutation_and_rejects_substitution() {
    let temp = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(temp.path()).unwrap();
    assert_eq!(
        storage
            .compare_and_swap_request_for_test("shop", "credit-one", "balance", None, "1")
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        storage
            .compare_and_swap_request_for_test("shop", "credit-one", "balance", None, "1")
            .unwrap(),
        Some(1)
    );
    assert!(
        storage
            .compare_and_swap_request_for_test("shop", "credit-one", "balance", None, "2")
            .is_err()
    );
    assert_eq!(storage.get("shop", "balance"), Some(("1".to_owned(), 1)));
    drop(storage);

    let mut storage = PluginStorage::open(temp.path()).unwrap();
    assert_eq!(
        storage
            .compare_and_swap_request_for_test("shop", "credit-one", "balance", None, "1")
            .unwrap(),
        Some(1)
    );
    assert_eq!(storage.get("shop", "balance"), Some(("1".to_owned(), 1)));
}

#[test]
fn storage_rejects_record_and_live_value_quota_without_mutating_memory() {
    let records = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(records.path()).unwrap();
    storage.fill_plugin_record_quota_for_test("shop");
    assert!(
        storage
            .compare_and_swap("shop", "one-too-many", None, "x")
            .is_err()
    );
    assert_eq!(storage.get("shop", "one-too-many"), None);

    let bytes = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(bytes.path()).unwrap();
    storage.set_live_bytes_for_test(64 * 1024 * 1024);
    assert!(
        storage
            .compare_and_swap("shop", "balance", None, "x")
            .is_err()
    );
    assert_eq!(storage.get("shop", "balance"), None);
}

#[test]
fn storage_truncates_only_an_incomplete_final_frame_after_a_valid_prefix() {
    let temp = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(temp.path()).unwrap();
    storage
        .compare_and_swap("shop", "balance", None, "1")
        .unwrap();
    drop(storage);

    let journal = journal_path(temp.path());
    let valid_length = std::fs::metadata(&journal).unwrap().len();
    use std::io::Write as _;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&journal)
        .unwrap()
        .write_all(&[4, 0])
        .unwrap();

    let storage = PluginStorage::open(temp.path()).unwrap();
    assert_eq!(storage.get("shop", "balance"), Some(("1".to_owned(), 1)));
    assert_eq!(std::fs::metadata(journal).unwrap().len(), valid_length);
}

#[test]
fn storage_fails_closed_for_checksum_and_oversized_frames() {
    let checksum = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(checksum.path()).unwrap();
    storage
        .compare_and_swap("shop", "balance", None, "1")
        .unwrap();
    drop(storage);
    let journal = journal_path(checksum.path());
    let mut bytes = std::fs::read(&journal).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    std::fs::write(&journal, bytes).unwrap();
    assert!(PluginStorage::open(checksum.path()).is_err());

    let oversized = tempfile::tempdir().unwrap();
    let journal = journal_path(oversized.path());
    std::fs::create_dir_all(journal.parent().unwrap()).unwrap();
    std::fs::write(&journal, u32::to_le_bytes(8_193)).unwrap();
    assert!(PluginStorage::open(oversized.path()).is_err());
}

#[test]
fn failed_append_or_rename_keeps_the_old_state_after_restart() {
    for fault in [StorageFaultPoint::Write, StorageFaultPoint::Rename] {
        let temp = tempfile::tempdir().unwrap();
        let mut storage = PluginStorage::open(temp.path()).unwrap();
        storage
            .compare_and_swap("shop", "balance", None, "1")
            .unwrap();
        storage.inject_fault_for_test(fault);
        let failed = if fault == StorageFaultPoint::Rename {
            storage.force_compact_for_test()
        } else {
            storage
                .compare_and_swap("shop", "balance", Some(1), "2")
                .map(drop)
        };
        assert!(failed.is_err());
        assert_eq!(storage.get("shop", "balance"), Some(("1".to_owned(), 1)));
        drop(storage);
        let storage = PluginStorage::open(temp.path()).unwrap();
        assert_eq!(storage.get("shop", "balance"), Some(("1".to_owned(), 1)));
    }
}

#[test]
fn sync_failure_after_append_replays_the_complete_transaction_on_restart() {
    let temp = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(temp.path()).unwrap();
    storage
        .compare_and_swap("shop", "balance", None, "1")
        .unwrap();
    storage.inject_fault_for_test(StorageFaultPoint::Sync);

    assert!(
        storage
            .compare_and_swap("shop", "balance", Some(1), "2")
            .is_err()
    );
    assert_eq!(storage.get("shop", "balance"), Some(("1".to_owned(), 1)));
    drop(storage);

    let mut storage = PluginStorage::open(temp.path()).unwrap();
    assert_eq!(storage.get("shop", "balance"), Some(("2".to_owned(), 2)));
    assert_eq!(storage.pending_result_count_for_test(), 1);
    storage.force_compact_for_test().unwrap();
    drop(storage);

    let storage = PluginStorage::open(temp.path()).unwrap();
    assert_eq!(storage.get("shop", "balance"), Some(("2".to_owned(), 2)));
    assert_eq!(storage.pending_result_count_for_test(), 1);
}

#[test]
fn result_ack_sync_failure_replays_the_complete_ack_on_restart() {
    let temp = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(temp.path()).unwrap();
    storage.inject_fault_for_test(StorageFaultPoint::ResultSync);

    assert!(
        storage
            .compare_and_swap("shop", "balance", None, "1")
            .is_err()
    );
    assert_eq!(storage.get("shop", "balance"), Some(("1".to_owned(), 1)));
    drop(storage);

    let storage = PluginStorage::open(temp.path()).unwrap();
    assert_eq!(storage.get("shop", "balance"), Some(("1".to_owned(), 1)));
    assert_eq!(storage.pending_result_count_for_test(), 0);
}

#[tokio::test]
async fn actor_durability_failure_accounts_for_current_and_queued_admissions_once() {
    let plugins = tempfile::tempdir().unwrap();
    let plugin = plugins.path().join("storage-failure");
    std::fs::create_dir(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"id = "storage-failure"
name = "Storage Failure"
version = "0.1.0"
api = "0.6.0"
events = ["server.started", "plugin.storage.cas_result"]
capabilities = ["storage"]
"#,
    )
    .unwrap();
    std::fs::write(
        plugin.join("main.lua"),
        r#"
--!strict

function on_server_started(_event: any)
    solaris.storage_cas("first", "key:first", nil, "1")
    solaris.storage_cas("second", "key:second", nil, "2")
    solaris.storage_cas("third", "key:third", nil, "3")
end

function on_plugin_storage_cas_result(event: any)
    solaris.broadcast(event.request_id .. ":" .. event.failure)
end
"#,
    )
    .unwrap();
    let (boundary, host) = start_lua_host(LuaHostConfig::new(plugins.path())).unwrap();
    assert_eq!(host.loaded_plugins(), 1);
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();

    let mut admitted = Vec::new();
    for _ in 0..3 {
        let command = boundary.recv_command().await.unwrap();
        admitted.push(boundary.accept_host_command(command).unwrap());
    }

    let world = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(world.path()).unwrap();
    storage.inject_fault_for_test(StorageFaultPoint::Write);
    run_storage_actor_for_test(
        storage,
        admitted,
        ScriptEventSink::new(boundary.clone()),
        ShutdownHandle::default(),
    )
    .await;

    for expected in ["first", "second", "third"] {
        let command = boundary.recv_command().await.unwrap();
        let admitted = boundary.accept_host_command(command).unwrap();
        assert_eq!(admitted.plugin_id(), "storage-failure");
        assert!(matches!(
            admitted.request(),
            ScriptCommand::BroadcastChatMessage { message }
                if message == &format!("{expected}:durability_failed")
        ));
    }

    let storage = PluginStorage::open(world.path()).unwrap();
    assert_eq!(storage.get("storage-failure", "key:first"), None);
    assert_eq!(storage.get("storage-failure", "key:second"), None);
    assert_eq!(storage.get("storage-failure", "key:third"), None);

    drop(boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn actor_sync_unknown_defers_current_and_fails_queued_before_restart_replay() {
    let plugins = tempfile::tempdir().unwrap();
    let plugin = plugins.path().join("storage-sync-unknown");
    std::fs::create_dir(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"id = "storage-sync-unknown"
name = "Storage Sync Unknown"
version = "0.1.0"
api = "0.6.0"
events = ["server.started", "plugin.storage.cas_result"]
capabilities = ["storage"]
"#,
    )
    .unwrap();
    std::fs::write(
        plugin.join("main.lua"),
        r#"
--!strict

function on_server_started(_event: any)
    solaris.storage_cas("first", "key:first", nil, "1")
    solaris.storage_cas("second", "key:second", nil, "2")
    solaris.storage_cas("third", "key:third", nil, "3")
end

function on_plugin_storage_cas_result(event: any)
    if event.failure then
        solaris.broadcast(event.request_id .. ":" .. event.failure)
    else
        solaris.broadcast(event.request_id .. ":committed:" .. event.version)
    end
end
"#,
    )
    .unwrap();
    let (boundary, host) = start_lua_host(LuaHostConfig::new(plugins.path())).unwrap();
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let mut admitted = Vec::new();
    for _ in 0..3 {
        let command = boundary.recv_command().await.unwrap();
        admitted.push(boundary.accept_host_command(command).unwrap());
    }

    let world = tempfile::tempdir().unwrap();
    let mut storage = PluginStorage::open(world.path()).unwrap();
    storage.inject_fault_for_test(StorageFaultPoint::Sync);
    run_storage_actor_for_test(
        storage,
        admitted,
        ScriptEventSink::new(boundary.clone()),
        ShutdownHandle::default(),
    )
    .await;

    for expected in ["second", "third"] {
        let command = boundary.recv_command().await.unwrap();
        let admitted = boundary.accept_host_command(command).unwrap();
        assert!(matches!(
            admitted.request(),
            ScriptCommand::BroadcastChatMessage { message }
                if message == &format!("{expected}:durability_failed")
        ));
    }
    let storage = PluginStorage::open(world.path()).unwrap();
    assert_eq!(
        storage.get("storage-sync-unknown", "key:first"),
        Some(("1".to_owned(), 1))
    );
    assert_eq!(storage.pending_result_count_for_test(), 1);

    run_storage_actor_for_test(
        storage,
        Vec::new(),
        ScriptEventSink::new(boundary.clone()),
        ShutdownHandle::default(),
    )
    .await;
    let command = boundary.recv_command().await.unwrap();
    let admitted = boundary.accept_host_command(command).unwrap();
    assert!(matches!(
        admitted.request(),
        ScriptCommand::BroadcastChatMessage { message } if message == "first:committed:1"
    ));
    assert_eq!(
        PluginStorage::open(world.path())
            .unwrap()
            .pending_result_count_for_test(),
        0
    );

    drop(boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn committed_result_survives_closed_delivery_and_replays_once_after_restart() {
    let plugins = tempfile::tempdir().unwrap();
    let plugin = plugins.path().join("storage-outbox");
    std::fs::create_dir(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"id = "storage-outbox"
name = "Storage Outbox"
version = "0.1.0"
api = "0.6.0"
events = ["server.started"]
capabilities = ["storage"]
"#,
    )
    .unwrap();
    std::fs::write(
        plugin.join("main.lua"),
        r#"
--!strict

function on_server_started(_event: any)
    solaris.storage_cas("commit-one", "balance", nil, "7")
end
"#,
    )
    .unwrap();
    let (lua_boundary, host) = start_lua_host(LuaHostConfig::new(plugins.path())).unwrap();
    lua_boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let command = lua_boundary.recv_command().await.unwrap();
    let admitted = lua_boundary.accept_host_command(command).unwrap();

    let (closed_boundary, closed_endpoint) = mc_script::script_boundary_pair(
        std::num::NonZeroUsize::new(1).unwrap(),
        std::num::NonZeroUsize::new(1).unwrap(),
    );
    drop(closed_endpoint);
    let world = tempfile::tempdir().unwrap();
    run_storage_actor_for_test(
        PluginStorage::open(world.path()).unwrap(),
        vec![admitted],
        ScriptEventSink::new(closed_boundary),
        ShutdownHandle::default(),
    )
    .await;

    let storage = PluginStorage::open(world.path()).unwrap();
    assert_eq!(
        storage.get("storage-outbox", "balance"),
        Some(("7".to_owned(), 1))
    );
    assert_eq!(storage.pending_result_count_for_test(), 1);

    let (replay_boundary, mut replay_endpoint) = mc_script::script_boundary_pair(
        std::num::NonZeroUsize::new(1).unwrap(),
        std::num::NonZeroUsize::new(1).unwrap(),
    );
    run_storage_actor_for_test(
        storage,
        Vec::new(),
        ScriptEventSink::new(replay_boundary),
        ShutdownHandle::default(),
    )
    .await;
    let replayed = replay_endpoint.recv_event().await.unwrap();
    assert_eq!(replayed.target_plugin_id(), Some("storage-outbox"));
    assert!(matches!(
        replayed.kind(),
        mc_script::ScriptEventKind::PluginStorageCasResult {
            request_id,
            key,
            applied: true,
            version: Some(1),
            failure: None,
        } if request_id == "commit-one" && key == "balance"
    ));

    let storage = PluginStorage::open(world.path()).unwrap();
    assert_eq!(storage.pending_result_count_for_test(), 0);
    drop(lua_boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn router_returns_explicit_unavailable_result_without_a_persistent_world() {
    let plugins = tempfile::tempdir().unwrap();
    let plugin = plugins.path().join("no-world");
    std::fs::create_dir(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"id = "no-world"
name = "No World"
version = "0.1.0"
api = "0.6.0"
events = ["server.started", "plugin.storage.get_result"]
capabilities = ["storage"]
"#,
    )
    .unwrap();
    std::fs::write(
        plugin.join("main.lua"),
        r#"
--!strict

function on_server_started(_event: any)
    solaris.storage_get("read", "balance")
end

function on_plugin_storage_get_result(event: any)
    solaris.broadcast(event.request_id .. ":" .. event.failure)
end
"#,
    )
    .unwrap();
    let (boundary, host) = start_lua_host(LuaHostConfig::new(plugins.path())).unwrap();
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let command = boundary.recv_command().await.unwrap();
    let admitted = boundary.accept_host_command(command).unwrap();
    let router = ScriptRouter::new(ScriptEventSink::new(boundary.clone()), None);

    assert_eq!(
        router
            .route_storage_admitted(admitted, &ShutdownHandle::default())
            .await,
        ScriptRouterExit::Continue
    );
    let command = boundary.recv_command().await.unwrap();
    let admitted = boundary.accept_host_command(command).unwrap();
    assert!(matches!(
        admitted.request(),
        ScriptCommand::BroadcastChatMessage { message } if message == "read:unavailable"
    ));

    drop(router);
    drop(boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn router_rejects_inventory_transaction_without_persistent_storage() {
    let plugins = tempfile::tempdir().unwrap();
    let plugin = plugins.path().join("no-world-transaction");
    std::fs::create_dir(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"id = "no-world-transaction"
name = "No World Transaction"
version = "0.1.0"
api = "0.6.0"
events = ["server.started", "inventory.storage_transaction.result"]
capabilities = ["inventory_storage_transactions"]
"#,
    )
    .unwrap();
    std::fs::write(
        plugin.join("main.lua"),
        r#"
--!strict

function on_server_started(_event: any)
    solaris.inventory_storage_transaction(
        1,
        "purchase",
        { { resource = "minecraft:apple", delta = 1 } },
        { { operation = "cas", key = "balance", expected_version = nil, value = "1" } }
    )
end

function on_inventory_storage_transaction_result(event: any)
    solaris.broadcast(event.request_id .. ":" .. tostring(event.committed))
end
"#,
    )
    .unwrap();
    let (boundary, host) = start_lua_host(LuaHostConfig::new(plugins.path())).unwrap();
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let command = boundary.recv_command().await.unwrap();
    let admitted = boundary.accept_host_command(command).unwrap();
    let router = ScriptRouter::new(ScriptEventSink::new(boundary.clone()), None);

    assert_eq!(
        router
            .route_storage_admitted(admitted, &ShutdownHandle::default())
            .await,
        ScriptRouterExit::Continue
    );
    let command = boundary.recv_command().await.unwrap();
    let admitted = boundary.accept_host_command(command).unwrap();
    assert!(matches!(
        admitted.request(),
        ScriptCommand::BroadcastChatMessage { message } if message == "purchase:false"
    ));

    drop(router);
    drop(boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .unwrap()
        .unwrap();
}

fn storage_bind_config(root: &std::path::Path) -> ServerConfig {
    std::fs::create_dir_all(root.join("dimensions/minecraft/overworld/region")).unwrap();
    let blocks = std::sync::Arc::new(mc_world::BlockRegistry::from_report(&[]).unwrap());
    let world = std::sync::Arc::new(tokio::sync::Mutex::new(
        mc_world::WorldStorage::open(root, std::sync::Arc::clone(&blocks)).unwrap(),
    ));
    ServerConfig {
        bind_address: "127.0.0.1:0".parse().unwrap(),
        motd: "storage startup test".to_owned(),
        max_players: 1,
        view_distance: 2,
        data: std::sync::Arc::new(mc_data::testing::stub()),
        blocks,
        world: Some(world),
        tags: std::sync::Arc::new(mc_data::tags::TagsData::default()),
        recipes: std::sync::Arc::new(Vec::new()),
        loot: std::sync::Arc::new(mc_data::loot::LootTables::default()),
        block_light: None,
        items: std::sync::Arc::new(mc_data::items::ItemRegistry::default()),
        item_facts: std::sync::Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: std::sync::Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types: std::sync::Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        biome_spawns: std::sync::Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: crate::ChunkPipelinePolicy::default(),
        random_tick: crate::play::RandomTickPolicy::default(),
        command_permissions: CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: ShutdownHandle::default(),
    }
}

#[tokio::test]
async fn bind_propagates_typed_malformed_and_io_storage_startup_failures() {
    for io_failure in [false, true] {
        let world = tempfile::tempdir().unwrap();
        let config = storage_bind_config(world.path());
        let storage_directory = world.path().join("solaris/plugin-storage-v1");
        std::fs::create_dir_all(storage_directory.parent().unwrap()).unwrap();
        if io_failure {
            std::fs::write(&storage_directory, b"not a directory").unwrap();
        } else {
            std::fs::create_dir(&storage_directory).unwrap();
            std::fs::write(
                storage_directory.join("journal-v1.bin"),
                u32::to_le_bytes(8_193),
            )
            .unwrap();
        }
        let (boundary, _endpoint) = mc_script::script_boundary_pair(
            std::num::NonZeroUsize::new(1).unwrap(),
            std::num::NonZeroUsize::new(1).unwrap(),
        );

        let error = match bind_with_scripts(config, boundary).await {
            Ok(_) => panic!("bind unexpectedly accepted broken plugin storage"),
            Err(error) => error,
        };
        let storage_error = error
            .get_ref()
            .and_then(|source| source.downcast_ref::<crate::PluginStorageStartError>())
            .expect("bind error must retain the typed plugin storage source");
        assert!(matches!(
            (io_failure, storage_error),
            (true, crate::PluginStorageStartError::Io(_))
                | (
                    false,
                    crate::PluginStorageStartError::Malformed("frame length")
                )
        ));
    }
}
