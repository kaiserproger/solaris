use std::collections::HashSet;
use std::future::Future;
use std::sync::{Arc, Mutex, mpsc as std_mpsc};
use std::task::Poll;

use mc_data::Identifier;
use mc_protocol::packets::play::ItemStack;
use mc_script::{
    ScriptInventoryResourceDelta, ScriptInventoryStorageTransaction, ScriptPlayerId,
    ScriptPlayerInventoryFailure, ScriptPlayerInventoryTransaction, ScriptStorageMutation,
};
use tokio::sync::mpsc;

use crate::login::LoggedInProfile;
use crate::play::inventory::PlayerInventory;
use crate::play::persistence::PlayerPersistedState;
use crate::play::script_inventory_transaction::{
    ScriptStoragePrepareOutcome, ScriptStorageTransactionPrepare,
};
use crate::play::{PlayerPose, SessionRegistry};

use super::EntityApplyReleaseProbe;
use super::outbound::OutboundCommand;
use super::script_player_inventory_endpoint::apply_script_player_inventory_transaction;

struct SignalingStorage {
    prepared: std_mpsc::Sender<()>,
}

impl ScriptStorageTransactionPrepare for SignalingStorage {
    type Prepared = ();
    type Error = std::convert::Infallible;

    fn prepare(
        &mut self,
        _plugin_id: &str,
        _mutations: &[ScriptStorageMutation],
    ) -> Result<ScriptStoragePrepareOutcome<Self::Prepared>, Self::Error> {
        self.prepared
            .send(())
            .expect("compound prepare observer remains present");
        Ok(ScriptStoragePrepareOutcome::Prepared(()))
    }

    fn commit(&mut self, _prepared: Self::Prepared) -> Result<(), Self::Error> {
        Ok(())
    }
}

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
    mpsc::Receiver<OutboundCommand>,
) {
    let registry = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("InventoryPluginTest"),
        name: "InventoryPluginTest".to_owned(),
    };
    let (outbound, receiver) = mpsc::channel(8);
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

#[tokio::test]
async fn player_inventory_transaction_waits_for_exact_session_owner_commit() {
    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let apple = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let emerald = items
        .id_of(&Identifier::parse("minecraft:emerald").unwrap())
        .unwrap();
    let mut initial = PlayerInventory::empty();
    initial.slots[9] = ItemStack::new(apple, 3);
    let (registry, player_id, persisted, mut outbound) = register_player(initial.clone());
    let request = transaction(
        player_id,
        "exchange",
        vec![
            ScriptInventoryResourceDelta::try_new("minecraft:apple", -2).unwrap(),
            ScriptInventoryResourceDelta::try_new("minecraft:emerald", 4).unwrap(),
        ],
    );
    let mut routed = Box::pin(registry.route_script_player_inventory_transaction(request));

    std::future::poll_fn(|context| {
        assert!(routed.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    assert_eq!(
        persisted.lock().unwrap().inventory.slots,
        initial.slots,
        "router must not mutate player persistence before the session owner"
    );
    let OutboundCommand::ScriptPlayerInventoryTransaction(command) = outbound.recv().await.unwrap()
    else {
        panic!("expected session-owner inventory transaction");
    };
    let mut owner_inventory = initial;
    let result = {
        let _transaction_guard = command.begin_commit().expect("session remains active");
        let mut saved = persisted.lock().unwrap();
        apply_script_player_inventory_transaction(
            command.transaction(),
            &mut owner_inventory,
            &mut saved,
            &items,
            &facts,
        )
    };
    command.complete(result);

    assert_eq!(routed.await, Ok(()));
    assert_eq!(owner_inventory.slots[9], ItemStack::new(apple, 1));
    assert!(
        owner_inventory.slots[9..=44]
            .iter()
            .any(|stack| *stack == ItemStack::new(emerald, 4))
    );
    assert_eq!(
        persisted.lock().unwrap().inventory.slots,
        owner_inventory.slots
    );
}

#[tokio::test]
async fn compound_transaction_waits_for_earlier_session_owner_inventory_commit() {
    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let apple = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let emerald = items
        .id_of(&Identifier::parse("minecraft:emerald").unwrap())
        .unwrap();
    let (registry, player_id, persisted, mut outbound) = register_player(PlayerInventory::empty());
    let registry = Arc::new(registry);
    let mut standalone = Box::pin(
        registry.route_script_player_inventory_transaction(transaction(
            player_id,
            "standalone-first",
            vec![ScriptInventoryResourceDelta::try_new("minecraft:apple", 1).unwrap()],
        )),
    );
    std::future::poll_fn(|context| {
        assert!(standalone.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    let OutboundCommand::ScriptPlayerInventoryTransaction(command) = outbound.recv().await.unwrap()
    else {
        panic!("expected standalone owner transaction");
    };

    let (captured, captured_receiver) = std_mpsc::channel();
    let (resume, resume_receiver) = std_mpsc::channel();
    *registry
        .script_transaction_capture_probe
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(EntityApplyReleaseProbe {
        reached: captured,
        resume: resume_receiver,
    });
    let compound = ScriptInventoryStorageTransaction::try_new(
        "compound-second",
        ScriptPlayerId::new(player_id),
        vec![
            ScriptInventoryResourceDelta::try_new("minecraft:apple", -1).unwrap(),
            ScriptInventoryResourceDelta::try_new("minecraft:emerald", 1).unwrap(),
        ],
        vec![ScriptStorageMutation::compare_and_swap("balance", None, "1").unwrap()],
    )
    .unwrap();
    let (prepared, prepared_receiver) = std_mpsc::channel();
    let compound_registry = Arc::clone(&registry);
    let compound_thread = std::thread::spawn(move || {
        compound_registry
            .commit_script_inventory_storage_transaction(
                "shop",
                &compound,
                &mc_data::items::solaris_required_items(),
                &mc_data::item_components::solaris_required_item_facts(),
                &mut SignalingStorage { prepared },
            )
            .unwrap()
    });
    captured_receiver
        .recv()
        .expect("compound transaction captured the shared gate");
    resume.send(()).expect("resume compound transaction");
    assert!(
        prepared_receiver.try_recv().is_err(),
        "compound planning must wait for the earlier owner reservation"
    );

    let mut owner_inventory = PlayerInventory::empty();
    let result = {
        let _transaction_guard = command.begin_commit().expect("session remains active");
        let mut saved = persisted.lock().unwrap();
        apply_script_player_inventory_transaction(
            command.transaction(),
            &mut owner_inventory,
            &mut saved,
            &items,
            &facts,
        )
    };
    command.complete(result);
    assert_eq!(standalone.await, Ok(()));
    prepared_receiver
        .recv()
        .expect("compound transaction reached storage after owner commit");
    assert!(compound_thread.join().expect("compound transaction thread"));

    let OutboundCommand::AuthoritativeInventory { inventory, .. } = outbound.recv().await.unwrap()
    else {
        panic!("expected compound authoritative inventory publication");
    };
    owner_inventory = *inventory;
    let saved = persisted.lock().unwrap();
    assert_eq!(owner_inventory.slots, saved.inventory.slots);
    assert!(
        owner_inventory.slots[9..=44]
            .iter()
            .all(|stack| stack.item_id != apple || stack.is_empty())
    );
    assert!(
        owner_inventory.slots[9..=44]
            .iter()
            .any(|stack| *stack == ItemStack::new(emerald, 1))
    );
}

#[test]
fn owner_inventory_transaction_maps_failures_without_partial_mutation() {
    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let apple = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let mut initial = PlayerInventory::empty();
    initial.slots[9] = ItemStack::new(apple, 1);
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
        let mut owner_inventory = initial.clone();
        let mut persisted = PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5));
        persisted.inventory = initial.clone();
        assert_eq!(
            apply_script_player_inventory_transaction(
                &transaction(1, request_id, deltas),
                &mut owner_inventory,
                &mut persisted,
                &items,
                &facts,
            ),
            Err(expected)
        );
        assert_eq!(owner_inventory.slots, initial.slots);
        assert_eq!(persisted.inventory.slots, initial.slots);
    }
}

#[test]
fn owner_inventory_transaction_rejects_full_inventory_without_mutation() {
    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let apple = items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let mut initial = PlayerInventory::empty();
    for slot in 9..=44 {
        initial.slots[slot] = ItemStack::new(apple, 64);
    }
    let mut owner_inventory = initial.clone();
    let mut persisted = PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5));
    persisted.inventory = initial.clone();

    assert_eq!(
        apply_script_player_inventory_transaction(
            &transaction(
                1,
                "full",
                vec![ScriptInventoryResourceDelta::try_new("minecraft:emerald", 1).unwrap()],
            ),
            &mut owner_inventory,
            &mut persisted,
            &items,
            &facts,
        ),
        Err(ScriptPlayerInventoryFailure::InventoryFull)
    );
    assert_eq!(owner_inventory.slots, initial.slots);
    assert_eq!(persisted.inventory.slots, initial.slots);
}

#[tokio::test]
async fn player_inventory_transaction_rejects_missing_or_closed_session() {
    let missing = transaction(
        77,
        "missing",
        vec![ScriptInventoryResourceDelta::try_new("minecraft:apple", 1).unwrap()],
    );
    assert_eq!(
        SessionRegistry::new()
            .route_script_player_inventory_transaction(missing)
            .await,
        Err(ScriptPlayerInventoryFailure::PlayerUnavailable)
    );

    let (registry, player_id, persisted, receiver) = register_player(PlayerInventory::empty());
    drop(receiver);
    assert_eq!(
        registry
            .route_script_player_inventory_transaction(transaction(
                player_id,
                "closed",
                vec![ScriptInventoryResourceDelta::try_new("minecraft:apple", 1).unwrap()],
            ))
            .await,
        Err(ScriptPlayerInventoryFailure::PlayerUnavailable)
    );
    assert!(
        persisted.lock().unwrap().inventory.slots[9..=44]
            .iter()
            .all(ItemStack::is_empty)
    );
}

#[tokio::test]
async fn dropped_session_owner_command_reports_player_unavailable_without_mutation() {
    let (registry, player_id, persisted, mut outbound) = register_player(PlayerInventory::empty());
    let mut routed = Box::pin(
        registry.route_script_player_inventory_transaction(transaction(
            player_id,
            "disconnect",
            vec![ScriptInventoryResourceDelta::try_new("minecraft:apple", 1).unwrap()],
        )),
    );
    std::future::poll_fn(|context| {
        assert!(routed.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    let command = outbound.recv().await.unwrap();
    assert!(matches!(
        command,
        OutboundCommand::ScriptPlayerInventoryTransaction(_)
    ));
    drop(command);

    assert_eq!(
        routed.await,
        Err(ScriptPlayerInventoryFailure::PlayerUnavailable)
    );
    assert!(
        persisted.lock().unwrap().inventory.slots[9..=44]
            .iter()
            .all(ItemStack::is_empty)
    );
}
