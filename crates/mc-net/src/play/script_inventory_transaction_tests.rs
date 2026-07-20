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
use super::script_inventory_transaction::{
    ScriptInventoryPlanError, ScriptStoragePrepareOutcome, ScriptStorageTransactionPrepare,
    plan_script_inventory_transaction,
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
    type Error = std::convert::Infallible;

    fn prepare(
        &mut self,
        _plugin_id: &str,
        _mutations: &[ScriptStorageMutation],
    ) -> Result<ScriptStoragePrepareOutcome<Self::Prepared>, Self::Error> {
        panic!("storage prepare must not run for a disconnected player")
    }

    fn commit(&mut self, _prepared: Self::Prepared) -> Result<(), Self::Error> {
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

    assert_eq!(plan.updated.slots[8].count, 64);
    assert_eq!(plan.updated.slots[9].count, 1);
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
