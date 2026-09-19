use mc_script::ScriptStorageMutation;

use super::storage::{PluginStorage, StorageFaultPoint};
use crate::server::{CommandPermissionConfig, ServerConfig, ShutdownHandle, bind_with_scripts};

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

fn storage_bind_config(root: &std::path::Path) -> ServerConfig {
    std::fs::create_dir_all(root.join("dimensions/minecraft/overworld/region")).unwrap();
    let blocks = std::sync::Arc::new(mc_world::BlockRegistry::from_report(&[]).unwrap());
    let world = std::sync::Arc::new(tokio::sync::Mutex::new(
        mc_world::WorldStorage::open(root, std::sync::Arc::clone(&blocks)).unwrap(),
    ));
    ServerConfig {
        tab_list: crate::server::TabListConfig::default(),
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
