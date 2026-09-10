use mc_data::Identifier;
use mc_data::item_components::solaris_required_item_facts;
use mc_data::items::solaris_required_items;
use mc_protocol::packets::play::ItemStack;
use mc_script::{
    ScriptInventoryResourceDelta, ScriptInventoryStorageTransaction, ScriptPlayerId,
    ScriptStorageMutation,
};

use super::SessionRegistry;
use super::inventory::PlayerInventory;
use super::persistence::inventory_recovery::PlayerInventoryRecovery;
use super::script_inventory_transaction::{
    ScriptInventoryPlanError, ScriptStorageCommitError, ScriptStoragePrepareOutcome,
    ScriptStorageTransactionPrepare, plan_script_inventory_transaction,
};

fn transaction(deltas: Vec<ScriptInventoryResourceDelta>) -> ScriptInventoryStorageTransaction {
    ScriptInventoryStorageTransaction::try_new(
        "purchase",
        ScriptPlayerId::new(7),
        deltas,
        vec![ScriptStorageMutation::compare_and_swap("coins:7", Some(1), "2").unwrap()],
    )
    .unwrap()
}

struct StorageMustNotRun;

impl ScriptStorageTransactionPrepare for StorageMustNotRun {
    type Prepared = ();
    type Error = std::io::Error;

    fn prepare(
        &mut self,
        _plugin_id: &str,
        _mutations: &[ScriptStorageMutation],
        _inventory: PlayerInventoryRecovery,
    ) -> Result<ScriptStoragePrepareOutcome<Self::Prepared>, Self::Error> {
        panic!("storage prepare must not run for a disconnected player")
    }

    fn commit(
        &mut self,
        _prepared: Self::Prepared,
    ) -> Result<u64, ScriptStorageCommitError<Self::Error>> {
        panic!("storage commit must not run for a disconnected player")
    }
}

#[test]
fn transaction_grant_and_remove_plan_only_player_slots() {
    let items = solaris_required_items();
    let facts = solaris_required_item_facts();
    let apple = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let mut inventory = PlayerInventory::empty();
    inventory.slots[8] = ItemStack::new(apple, 64);
    inventory.slots[9] = ItemStack::new(apple, 2);

    let plan = plan_script_inventory_transaction(
        &transaction(vec![
            ScriptInventoryResourceDelta::try_new("minecraft:apple", -1).unwrap(),
        ]),
        &inventory,
        &items,
        &facts,
    )
    .unwrap();

    assert_eq!(plan.slots[8].count, 64);
    assert_eq!(plan.slots[9].count, 1);
}

#[test]
fn transaction_rejects_insufficient_full_and_unknown_resources_without_a_plan() {
    let items = solaris_required_items();
    let facts = solaris_required_item_facts();
    let apple = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let inventory = PlayerInventory::empty();

    assert!(matches!(
        plan_script_inventory_transaction(
            &transaction(vec![
                ScriptInventoryResourceDelta::try_new("minecraft:apple", -1).unwrap()
            ]),
            &inventory,
            &items,
            &facts,
        ),
        Err(ScriptInventoryPlanError::InsufficientResource(_))
    ));
    assert!(matches!(
        plan_script_inventory_transaction(
            &transaction(vec![
                ScriptInventoryResourceDelta::try_new("minecraft:nope", 1).unwrap()
            ]),
            &inventory,
            &items,
            &facts,
        ),
        Err(ScriptInventoryPlanError::UnknownResource(_))
    ));

    let mut full = PlayerInventory::empty();
    for slot in 9..=44 {
        full.slots[slot] = ItemStack::new(apple, 64);
    }
    assert!(matches!(
        plan_script_inventory_transaction(
            &transaction(vec![
                ScriptInventoryResourceDelta::try_new("minecraft:apple", 1).unwrap()
            ]),
            &full,
            &items,
            &facts,
        ),
        Err(ScriptInventoryPlanError::InventoryFull(_))
    ));
}

#[test]
fn transaction_rejects_disconnected_player_before_touching_storage() {
    let items = solaris_required_items();
    let facts = solaris_required_item_facts();
    let transaction = ScriptInventoryStorageTransaction::try_new(
        "missing-player",
        ScriptPlayerId::new(7),
        vec![ScriptInventoryResourceDelta::try_new("minecraft:apple", 1).unwrap()],
        vec![ScriptStorageMutation::compare_and_swap("balance", None, "1").unwrap()],
    )
    .unwrap();

    let committed = SessionRegistry::new()
        .commit_script_inventory_storage_transaction(
            "shop",
            &transaction,
            &items,
            &facts,
            &mut StorageMustNotRun,
        )
        .unwrap();

    assert!(!committed);
}

#[tokio::test]
async fn compound_transaction_recovers_inventory_after_process_crash() {
    use std::io::{BufRead, Read, Write};
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};

    use crate::login::LoggedInProfile;
    use crate::script::storage::world_inventory::InventoryRuntime;
    use crate::script::storage::{PluginStorage, PluginStorageMutationError, StorageFaultPoint};

    use super::PlayerPose;
    use super::persistence::{PlayerPersistedState, load_player_state, save_player_state};

    const CHILD_WORLD: &str = "SOLARIS_C1_CRASH_WORLD";
    const BOUNDARY: &str = "SOLARIS_C1_CRASH_BOUNDARY";
    const COMMITTED: &str = "C1_WORLD_INVENTORY_DECISION_SYNCED";
    let items = Arc::new(solaris_required_items());
    let facts = Arc::new(solaris_required_item_facts());
    let apple = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let uuid = crate::login::offline_uuid("C1Recovery");
    let pose = PlayerPose::new(0.5, 64.0, 0.5);

    if let Some(root) = std::env::var_os(CHILD_WORLD) {
        let root = std::path::Path::new(&root);
        let mut saved = PlayerPersistedState::new_default(pose);
        saved.inventory.slots[9] = ItemStack::new(apple, 3);
        save_player_state(root, uuid, &items, &saved).unwrap();
        let mut storage = PluginStorage::open(root).unwrap();
        let sessions = Arc::new(SessionRegistry::new());
        let inventory = InventoryRuntime::player_only_for_test(
            root,
            Arc::clone(&sessions),
            Arc::clone(&items),
            Arc::clone(&facts),
        );
        let journal = sessions.world_chunk_journal().unwrap();
        journal.reserve_decision_ids(2).unwrap();
        journal
            .record_reserved_snapshot_groups(0, vec![(1, Vec::new()), (2, Vec::new())])
            .unwrap();
        let (outbound, _receiver) = tokio::sync::mpsc::channel(8);
        let (player_id, _) = sessions.register(
            &LoggedInProfile {
                uuid,
                name: "C1Recovery".to_owned(),
            },
            (0, 0),
            2,
            std::collections::HashSet::new(),
            outbound,
            pose,
        );
        sessions.register_player_persistence(player_id, Arc::new(Mutex::new(saved)));
        let purchase = ScriptInventoryStorageTransaction::try_new(
            "purchase",
            ScriptPlayerId::new(player_id),
            vec![ScriptInventoryResourceDelta::try_new("minecraft:apple", -1).unwrap()],
            vec![ScriptStorageMutation::compare_and_swap("paid:purchase", None, "1").unwrap()],
        )
        .unwrap();
        let boundary = std::env::var(BOUNDARY).unwrap();
        match boundary.as_str() {
            "world_synced" => storage.inject_fault_for_test(StorageFaultPoint::Write),
            "storage_appended" => storage.inject_fault_for_test(StorageFaultPoint::Sync),
            "fully_projected" => {}
            _ => panic!("unknown crash boundary"),
        }
        let result = inventory
            .commit_storage(&mut storage, "shop", &purchase)
            .await;
        if boundary == "fully_projected" {
            assert!(result.unwrap());
        } else {
            assert!(matches!(
                result,
                Err(PluginStorageMutationError::DurabilityUnknown(_))
            ));
        }
        println!("{COMMITTED}");
        std::io::stdout().flush().unwrap();
        let mut release = [0];
        std::io::stdin().read_exact(&mut release).unwrap();
        panic!("parent must terminate at the selected durable boundary");
    }

    for boundary in ["world_synced", "storage_appended", "fully_projected"] {
        let world = tempfile::tempdir().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "play::script_inventory_transaction_tests::compound_transaction_recovers_inventory_after_process_crash",
                "--nocapture",
            ])
            .env(CHILD_WORLD, world.path())
            .env(BOUNDARY, boundary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            if output.read_line(&mut line).unwrap() == 0 {
                let result = child.wait_with_output().unwrap();
                panic!("crash child exited before {boundary}: {result:?}");
            }
            if line.trim() == COMMITTED {
                break;
            }
        }
        child.kill().unwrap();
        assert!(!child.wait().unwrap().success());

        let inventory = InventoryRuntime::player_only_for_test(
            world.path(),
            Arc::new(SessionRegistry::new()),
            Arc::clone(&items),
            Arc::clone(&facts),
        );
        let mut storage = PluginStorage::open(world.path()).unwrap();
        inventory.recover(&mut storage).unwrap();
        assert_eq!(
            storage.get("shop", "paid:purchase"),
            Some(("1".to_owned(), 1))
        );
        let mut recovered = load_player_state(
            world.path(),
            uuid,
            &items,
            PlayerPersistedState::new_default(pose),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            recovered.inventory.slots[9],
            ItemStack::new(apple, 2),
            "{boundary}"
        );
        assert_eq!(recovered.inventory_operation_revision, 3);

        recovered.inventory.slots[9] = ItemStack::new(apple, 1);
        save_player_state(world.path(), uuid, &items, &recovered).unwrap();
        drop(storage);
        drop(inventory);
        let sessions = Arc::new(SessionRegistry::new());
        let inventory = InventoryRuntime::player_only_for_test(
            world.path(),
            Arc::clone(&sessions),
            Arc::clone(&items),
            Arc::clone(&facts),
        );
        let mut reopened = PluginStorage::open(world.path()).unwrap();
        inventory.recover(&mut reopened).unwrap();
        let later = load_player_state(
            world.path(),
            uuid,
            &items,
            PlayerPersistedState::new_default(pose),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            later.inventory.slots[9],
            ItemStack::new(apple, 1),
            "{boundary}"
        );
        let journal = sessions.world_chunk_journal().unwrap();
        let cutoff = journal
            .watermark()
            .expect("startup projection restores checkpoint eligibility");
        journal.checkpoint_through(cutoff).unwrap();
        drop(reopened);
        drop(journal);
        drop(inventory);
        drop(sessions);
        let sessions = Arc::new(SessionRegistry::new());
        let _inventory = InventoryRuntime::player_only_for_test(
            world.path(),
            Arc::clone(&sessions),
            Arc::clone(&items),
            Arc::clone(&facts),
        );
        assert!(
            !sessions
                .world_chunk_journal()
                .unwrap()
                .has_inventory_decisions(),
            "recovered decisions must not permanently retain later world writes"
        );
    }
}
