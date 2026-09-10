use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use mc_data::Identifier;
use mc_data::item_components::solaris_required_item_facts;
use mc_data::items::solaris_required_items;
use mc_entity::Vec3;
use mc_script::{
    ScriptInventoryResourceDelta, ScriptInventoryStorageTransaction, ScriptPlayerId,
    ScriptStorageMutation,
};

use super::{
    SelectedItemDropCommand, SelectedItemDropPlan, SimulationCommand, SimulationResponse,
    simulation_channel_with_capacity,
};
use crate::login::LoggedInProfile;
use crate::play::inventory::PlayerInventory;
use crate::play::persistence::{PlayerPersistedState, load_player_state, save_player_state};
use crate::play::{PlayerPose, SessionRegistry};
use crate::script::storage::world_inventory::InventoryRuntime;
use crate::script::storage::{PluginStorage, PluginStorageMutationError, StorageFaultPoint};

#[tokio::test]
async fn uncertain_inventory_commit_fences_queued_native_drop_before_recovery() {
    let world = tempfile::tempdir().unwrap();
    let items = Arc::new(solaris_required_items());
    let facts = Arc::new(solaris_required_item_facts());
    let apple = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let uuid = crate::login::offline_uuid("C1Uncertain");
    let pose = PlayerPose::new(0.5, 64.0, 0.5);
    let mut saved = PlayerPersistedState::new_default(pose);
    saved.inventory.slots[PlayerInventory::HOTBAR_BASE] = mc_data::ItemStack::new(apple, 3);
    save_player_state(world.path(), uuid, &items, &saved).unwrap();
    let player_state = Arc::new(Mutex::new(saved));
    let sessions = Arc::new(SessionRegistry::new());
    let inventory = InventoryRuntime::player_only_for_test(
        world.path(),
        Arc::clone(&sessions),
        Arc::clone(&items),
        Arc::clone(&facts),
    );
    let (outbound, _receiver) = tokio::sync::mpsc::channel(8);
    let (player_id, _) = sessions.register(
        &LoggedInProfile {
            uuid,
            name: "C1Uncertain".to_owned(),
        },
        (0, 0),
        2,
        HashSet::new(),
        outbound,
        pose,
    );
    sessions.register_player_persistence(player_id, Arc::clone(&player_state));
    let (handle, mut owner) = simulation_channel_with_capacity(1);
    let drop_response = handle
        .for_session(player_id)
        .enqueue_player_command(SimulationCommand::CommitSelectedItemDrop(
            SelectedItemDropCommand {
                actor_session: player_id,
                plan: SelectedItemDropPlan {
                    held_hotbar_slot: 0,
                    expected_held: mc_data::ItemStack::new(apple, 3),
                    drop_count: 1,
                    entity_type_id: 1,
                    position: Vec3::new(0.5, 65.0, 0.5),
                },
            },
        ))
        .unwrap();
    let purchase = ScriptInventoryStorageTransaction::try_new(
        "purchase",
        ScriptPlayerId::new(player_id),
        vec![ScriptInventoryResourceDelta::try_new("minecraft:apple", -1).unwrap()],
        vec![ScriptStorageMutation::compare_and_swap("paid:purchase", None, "1").unwrap()],
    )
    .unwrap();
    let mut storage = PluginStorage::open(world.path()).unwrap();
    storage.inject_fault_for_test(StorageFaultPoint::Sync);
    assert!(matches!(
        inventory
            .commit_storage(&mut storage, "shop", &purchase)
            .await,
        Err(PluginStorageMutationError::DurabilityUnknown(_))
    ));

    // The owner resumes after the journal error releases the player lock,
    // before the storage actor's asynchronous shutdown reaches this queue.
    assert_eq!(owner.process_tick(&sessions, 1).processed, 1);
    assert!(matches!(
        drop_response.await.unwrap().unwrap(),
        SimulationResponse::SelectedItemDrop(Ok(None))
    ));
    assert!(
        sessions
            .persisted_entity_records()
            .iter()
            .all(|record| record.snapshot.item_stack.is_none())
    );
    save_player_state(world.path(), uuid, &items, &player_state.lock().unwrap()).unwrap();
    drop(storage);
    drop(inventory);
    drop(sessions);
    let inventory = InventoryRuntime::player_only_for_test(
        world.path(),
        Arc::new(SessionRegistry::new()),
        Arc::clone(&items),
        Arc::clone(&facts),
    );
    let mut recovered_storage = PluginStorage::open(world.path()).unwrap();
    inventory.recover(&mut recovered_storage).unwrap();
    let recovered = load_player_state(
        world.path(),
        uuid,
        &items,
        PlayerPersistedState::new_default(pose),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        recovered_storage.get("shop", "paid:purchase"),
        Some(("1".to_owned(), 1))
    );
    assert_eq!(
        recovered.inventory.slots[PlayerInventory::HOTBAR_BASE],
        mc_data::ItemStack::new(apple, 2)
    );
}
