use mc_data::items::solaris_required_items;
use mc_data::{Identifier, ItemStack};

use super::super::{load_player_state, save_player_state};
use super::*;
use crate::play::PlayerPose;

fn replay_open_container_inventory(root: &Path, items: &ItemRegistry) -> PlayerPersistedState {
    let uuid = Uuid::from_u128(17);
    let apple = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let mut original = PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5));
    original.inventory.slots[9] = ItemStack::new(apple, 15);
    save_player_state(root, uuid, items, &original).unwrap();

    let mut live = original.clone();
    live.inventory.slots[9] = ItemStack::new(apple, 1);
    live.carried_item = ItemStack::new(apple, 2);
    live.carried_item.custom_name = Some("Shipment".to_owned());
    let mut crafting = std::array::from_fn(|_| ItemStack::default());
    crafting[0] = ItemStack::new(apple, 3);
    live.crafting_table_input = Some(Box::new(crafting));
    let mut enchanting = std::array::from_fn(|_| ItemStack::default());
    enchanting[0] = ItemStack::new(apple, 4);
    live.enchanting_table_input = Some(Box::new(enchanting));
    let mut merchant = std::array::from_fn(|_| ItemStack::default());
    merchant[0] = ItemStack::new(apple, 5);
    live.merchant_input = Some(Box::new(merchant));
    let mut planned = live.inventory.clone();
    planned.slots[9] = ItemStack::default();
    PlayerInventoryRecovery::capture(uuid, &live, &planned, items)
        .unwrap()
        .recover(root, 7)
        .unwrap();
    original
}

fn owned_item_count(state: &PlayerPersistedState) -> i32 {
    state
        .inventory
        .slots
        .iter()
        .map(|item| item.count)
        .sum::<i32>()
        + state.carried_item.count
        + state
            .crafting_table_input
            .iter()
            .flat_map(|slots| slots.iter())
            .map(|item| item.count)
            .sum::<i32>()
        + state
            .enchanting_table_input
            .iter()
            .flat_map(|slots| slots.iter())
            .map(|item| item.count)
            .sum::<i32>()
        + state
            .merchant_input
            .iter()
            .flat_map(|slots| slots.iter())
            .map(|item| item.count)
            .sum::<i32>()
}

#[test]
fn inventory_recovery_preserves_items_held_in_open_containers() {
    let root = tempfile::tempdir().unwrap();
    let items = solaris_required_items();
    replay_open_container_inventory(root.path(), &items);
    let default = PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5));
    let recovered = load_player_state(root.path(), Uuid::from_u128(17), &items, default)
        .unwrap()
        .unwrap();
    assert_eq!(owned_item_count(&recovered), 14);
    assert!(recovered.inventory.slots[9].is_empty());
    assert_eq!(
        recovered.carried_item.custom_name.as_deref(),
        Some("Shipment")
    );
}

#[test]
fn stale_save_preserves_recovered_inventory_but_updates_other_player_state() {
    let root = tempfile::tempdir().unwrap();
    let items = solaris_required_items();
    let mut stale = replay_open_container_inventory(root.path(), &items);
    stale.pose = PlayerPose::new(12.5, 64.0, 0.5);
    save_player_state(root.path(), Uuid::from_u128(17), &items, &stale).unwrap();
    let default = PlayerPersistedState::new_default(stale.pose);
    let recovered = load_player_state(root.path(), Uuid::from_u128(17), &items, default)
        .unwrap()
        .unwrap();
    assert_eq!(owned_item_count(&recovered), 14);
    assert_eq!(recovered.inventory_operation_revision, 7);
    assert_eq!(recovered.pose.x, 12.5);
}

#[tokio::test]
async fn inventory_recovery_runs_when_server_binds_without_scripts() {
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    use mc_script::{
        ScriptInventoryResourceDelta, ScriptInventoryStorageTransaction, ScriptPlayerId,
        ScriptStorageMutation,
    };

    use crate::login::LoggedInProfile;
    use crate::play::SessionRegistry;
    use crate::script::storage::world_inventory::InventoryRuntime;
    use crate::script::storage::{PluginStorage, PluginStorageMutationError, StorageFaultPoint};

    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("region")).unwrap();
    let items = Arc::new(solaris_required_items());
    let facts = Arc::new(mc_data::item_components::solaris_required_item_facts());
    let apple = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let uuid = Uuid::from_u128(23);
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let mut player = PlayerPersistedState::new_default(pose);
    player.inventory.slots[9] = ItemStack::new(apple, 3);
    save_player_state(root.path(), uuid, &items, &player).unwrap();
    let sessions = Arc::new(SessionRegistry::new());
    let inventory = InventoryRuntime::player_only_for_test(
        root.path(),
        Arc::clone(&sessions),
        Arc::clone(&items),
        facts,
    );
    let (outbound, _receiver) = tokio::sync::mpsc::channel(8);
    let (player_id, _) = sessions.register(
        &LoggedInProfile {
            uuid,
            name: "NoLuaRecovery".to_owned(),
        },
        (0, 0),
        2,
        HashSet::new(),
        outbound,
        pose,
    );
    sessions.register_player_persistence(player_id, Arc::new(Mutex::new(player)));
    let purchase = ScriptInventoryStorageTransaction::try_new(
        "purchase",
        ScriptPlayerId::new(player_id),
        vec![ScriptInventoryResourceDelta::try_new("minecraft:apple", -1).unwrap()],
        vec![ScriptStorageMutation::compare_and_swap("paid:purchase", None, "1").unwrap()],
    )
    .unwrap();
    let mut storage = PluginStorage::open(root.path()).unwrap();
    storage.inject_fault_for_test(StorageFaultPoint::Write);
    assert!(matches!(
        inventory
            .commit_storage(&mut storage, "shop", &purchase)
            .await,
        Err(PluginStorageMutationError::DurabilityUnknown(_))
    ));
    assert!(
        *sessions.subscribe_world_chunk_journal_failure().borrow(),
        "an unprojected durable inventory decision must stop world admission"
    );
    drop(storage);
    drop(inventory);
    drop(sessions);

    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .unwrap(),
    );
    let bound = crate::server::bind(crate::server::tests::save_all_test_config(
        root.path(),
        blocks,
        Arc::clone(&items),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
    ))
    .await
    .unwrap();
    let recovered = load_player_state(
        root.path(),
        uuid,
        &items,
        PlayerPersistedState::new_default(pose),
    )
    .unwrap()
    .unwrap();
    assert_eq!(recovered.inventory.slots[9], ItemStack::new(apple, 2));
    let storage = PluginStorage::open(root.path()).unwrap();
    assert_eq!(
        storage.get("shop", "paid:purchase"),
        Some(("1".to_owned(), 1))
    );
    drop(bound);
}
