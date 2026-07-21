use std::collections::HashSet;
use std::sync::{Arc, Mutex, mpsc};

use mc_data::Identifier;
use mc_protocol::packets::play::ItemStack;
use mc_script::{
    ScriptInventoryResourceDelta, ScriptPlayerId, ScriptPlayerInventoryFailure,
    ScriptPlayerInventoryTransaction,
};
use tokio::sync::mpsc as tokio_mpsc;

use crate::login::LoggedInProfile;
use crate::play::inventory::PlayerInventory;
use crate::play::persistence::PlayerPersistedState;
use crate::play::{PlayerPose, SessionRegistry};

use super::EntityApplyReleaseProbe;
use super::outbound::OutboundCommand;

fn transaction(
    player_id: u64,
    request_id: &str,
    deltas: Vec<ScriptInventoryResourceDelta>,
) -> ScriptPlayerInventoryTransaction {
    ScriptPlayerInventoryTransaction::try_new(request_id, ScriptPlayerId::new(player_id), deltas)
        .unwrap()
}

fn register_player(
    inventory: PlayerInventory,
) -> (
    SessionRegistry,
    u64,
    Arc<Mutex<PlayerPersistedState>>,
    tokio_mpsc::Receiver<OutboundCommand>,
) {
    let registry = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("InventoryPluginTest"),
        name: "InventoryPluginTest".to_owned(),
    };
    let (outbound, receiver) = tokio_mpsc::channel(8);
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        2,
        HashSet::new(),
        outbound,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    let mut persisted = PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5));
    persisted.inventory = inventory;
    let persisted = Arc::new(Mutex::new(persisted));
    registry.register_player_persistence(session_id, Arc::clone(&persisted));
    (registry, session_id, persisted, receiver)
}

#[test]
fn player_inventory_transaction_commits_once_and_publishes_authoritative_inventory() {
    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let apple = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let emerald = items
        .id_of(&Identifier::parse("minecraft:emerald").unwrap())
        .unwrap();
    let mut inventory = PlayerInventory::empty();
    inventory.slots[9] = ItemStack::new(apple, 3);
    let (registry, player_id, persisted, mut outbound) = register_player(inventory);

    let result = registry.commit_script_player_inventory_transaction(
        &transaction(
            player_id,
            "exchange",
            vec![
                ScriptInventoryResourceDelta::try_new("minecraft:apple", -2).unwrap(),
                ScriptInventoryResourceDelta::try_new("minecraft:emerald", 4).unwrap(),
            ],
        ),
        &items,
        &facts,
    );

    assert_eq!(result, Ok(()));
    let state = persisted
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    assert_eq!(state.inventory.slots[9], ItemStack::new(apple, 1));
    assert!(
        state.inventory.slots[9..=44]
            .iter()
            .any(|stack| *stack == ItemStack::new(emerald, 4))
    );
    drop(state);
    let command = outbound.try_recv().expect("authoritative inventory");
    assert!(matches!(
        command,
        OutboundCommand::AuthoritativeInventory { inventory, .. }
            if inventory.slots[9] == ItemStack::new(apple, 1)
                && inventory.slots[9..=44]
                    .iter()
                    .any(|stack| *stack == ItemStack::new(emerald, 4))
    ));
    assert!(outbound.try_recv().is_err());
}

#[test]
fn player_inventory_transaction_maps_planner_failures_without_partial_mutation() {
    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let apple = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let mut inventory = PlayerInventory::empty();
    inventory.slots[9] = ItemStack::new(apple, 1);
    let (registry, player_id, persisted, mut outbound) = register_player(inventory.clone());

    let cases = [
        (
            "unknown",
            vec![
                ScriptInventoryResourceDelta::try_new("minecraft:apple", -1).unwrap(),
                ScriptInventoryResourceDelta::try_new("minecraft:not_an_item", 1).unwrap(),
            ],
            ScriptPlayerInventoryFailure::UnknownResource,
        ),
        (
            "insufficient",
            vec![ScriptInventoryResourceDelta::try_new("minecraft:apple", -2).unwrap()],
            ScriptPlayerInventoryFailure::InsufficientResource,
        ),
    ];
    for (request_id, deltas, expected) in cases {
        assert_eq!(
            registry.commit_script_player_inventory_transaction(
                &transaction(player_id, request_id, deltas),
                &items,
                &facts,
            ),
            Err(expected)
        );
        assert_eq!(
            persisted
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .inventory
                .slots,
            inventory.slots
        );
        assert!(outbound.try_recv().is_err());
    }
}

#[test]
fn player_inventory_transaction_rejects_full_inventory_without_mutation() {
    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let apple = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let mut inventory = PlayerInventory::empty();
    for slot in 9..=44 {
        inventory.slots[slot] = ItemStack::new(apple, 64);
    }
    let (registry, player_id, persisted, mut outbound) = register_player(inventory.clone());

    assert_eq!(
        registry.commit_script_player_inventory_transaction(
            &transaction(
                player_id,
                "full",
                vec![ScriptInventoryResourceDelta::try_new("minecraft:emerald", 1).unwrap()],
            ),
            &items,
            &facts,
        ),
        Err(ScriptPlayerInventoryFailure::InventoryFull)
    );
    assert_eq!(
        persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .inventory
            .slots,
        inventory.slots
    );
    assert!(outbound.try_recv().is_err());
}

#[test]
fn player_inventory_transaction_rejects_missing_state_and_closed_client() {
    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let missing = transaction(
        77,
        "missing",
        vec![ScriptInventoryResourceDelta::try_new("minecraft:apple", 1).unwrap()],
    );
    assert_eq!(
        SessionRegistry::new()
            .commit_script_player_inventory_transaction(&missing, &items, &facts,),
        Err(ScriptPlayerInventoryFailure::PlayerUnavailable)
    );

    let registry = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("InventoryMissingState"),
        name: "InventoryMissingState".to_owned(),
    };
    let (outbound, _receiver) = tokio_mpsc::channel(8);
    let (player_id, _) = registry.register(
        &profile,
        (0, 0),
        2,
        HashSet::new(),
        outbound,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    assert_eq!(
        registry.commit_script_player_inventory_transaction(
            &transaction(
                player_id,
                "missing-state",
                vec![ScriptInventoryResourceDelta::try_new("minecraft:apple", 1).unwrap()],
            ),
            &items,
            &facts,
        ),
        Err(ScriptPlayerInventoryFailure::PlayerUnavailable)
    );

    let (registry, player_id, persisted, receiver) = register_player(PlayerInventory::empty());
    drop(receiver);
    assert_eq!(
        registry.commit_script_player_inventory_transaction(
            &transaction(
                player_id,
                "closed",
                vec![ScriptInventoryResourceDelta::try_new("minecraft:apple", 1).unwrap()],
            ),
            &items,
            &facts,
        ),
        Err(ScriptPlayerInventoryFailure::PlayerUnavailable)
    );
    assert!(
        persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .inventory
            .slots[9..=44]
            .iter()
            .all(ItemStack::is_empty)
    );
}

#[test]
fn unregister_after_player_inventory_capture_wins_lifetime_fence() {
    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let (registry, player_id, persisted, _outbound) = register_player(PlayerInventory::empty());
    let registry = Arc::new(registry);
    let (reached, reached_receiver) = mpsc::channel();
    let (resume, resume_receiver) = mpsc::channel();
    *registry
        .script_transaction_capture_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(EntityApplyReleaseProbe {
        reached,
        resume: resume_receiver,
    });
    let transaction = transaction(
        player_id,
        "disconnect",
        vec![ScriptInventoryResourceDelta::try_new("minecraft:apple", 1).unwrap()],
    );
    let transaction_registry = Arc::clone(&registry);
    let thread = std::thread::spawn(move || {
        transaction_registry.commit_script_player_inventory_transaction(
            &transaction,
            &items,
            &facts,
        )
    });

    reached_receiver
        .recv()
        .expect("transaction captured session");
    registry.unregister_preserving_player_state(player_id);
    resume.send(()).expect("resume transaction");

    assert_eq!(
        thread.join().expect("transaction thread"),
        Err(ScriptPlayerInventoryFailure::PlayerUnavailable)
    );
    assert!(
        persisted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .inventory
            .slots[9..=44]
            .iter()
            .all(ItemStack::is_empty)
    );
}
