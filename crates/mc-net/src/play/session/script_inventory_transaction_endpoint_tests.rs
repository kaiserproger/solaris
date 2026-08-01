use std::collections::HashSet;
use std::sync::{Arc, Mutex, mpsc};

use mc_script::{
    ScriptInventoryResourceDelta, ScriptInventoryStorageTransaction, ScriptPlayerId,
    ScriptStorageMutation,
};
use tokio::sync::mpsc as tokio_mpsc;

use crate::login::LoggedInProfile;
use crate::play::persistence::PlayerPersistedState;
use crate::play::script_inventory_transaction::{
    ScriptStoragePrepareOutcome, ScriptStorageTransactionPrepare,
};
use crate::play::{PlayerPose, SessionRegistry};

use super::EntityApplyReleaseProbe;

struct StorageMustNotRun;

impl ScriptStorageTransactionPrepare for StorageMustNotRun {
    type Prepared = ();
    type Error = std::convert::Infallible;

    fn prepare(
        &mut self,
        _plugin_id: &str,
        _mutations: &[ScriptStorageMutation],
    ) -> Result<ScriptStoragePrepareOutcome<Self::Prepared>, Self::Error> {
        panic!("storage prepare must not run after unregister wins the lifetime fence")
    }

    fn commit(&mut self, _prepared: Self::Prepared) -> Result<(), Self::Error> {
        panic!("storage commit must not run after unregister wins the lifetime fence")
    }
}

#[test]
fn unregister_after_transaction_capture_rejects_before_storage_commit() {
    let registry = Arc::new(SessionRegistry::new());
    let profile = LoggedInProfile {
        uuid: crate::login::offline_uuid("TransactionDisconnect"),
        name: "TransactionDisconnect".to_owned(),
    };
    let (outbound, _outbound_receiver) = tokio_mpsc::channel(8);
    let (session_id, _) = registry.register(
        &profile,
        (0, 0),
        2,
        HashSet::new(),
        outbound,
        PlayerPose::new(0.5, 64.0, 0.5),
    );
    registry.register_player_persistence(
        session_id,
        Arc::new(Mutex::new(PlayerPersistedState::new_default(
            PlayerPose::new(0.5, 64.0, 0.5),
        ))),
    );

    let (reached, reached_receiver) = mpsc::channel();
    let (resume, resume_receiver) = mpsc::channel();
    *registry
        .script_transaction_capture_probe
        .lock()
        .expect("test lock poisoned") = Some(EntityApplyReleaseProbe {
        reached,
        resume: resume_receiver,
    });

    let transaction = ScriptInventoryStorageTransaction::try_new(
        "disconnect-race",
        ScriptPlayerId::new(session_id),
        vec![ScriptInventoryResourceDelta::try_new("minecraft:apple", 1).unwrap()],
        vec![ScriptStorageMutation::compare_and_swap("balance", None, "1").unwrap()],
    )
    .unwrap();
    let transaction_registry = Arc::clone(&registry);
    let transaction_thread = std::thread::spawn(move || {
        transaction_registry
            .commit_script_inventory_storage_transaction(
                "shop",
                &transaction,
                &mc_data::items::solaris_required_items(),
                &mc_data::item_components::solaris_required_item_facts(),
                &mut StorageMustNotRun,
            )
            .unwrap()
    });

    reached_receiver
        .recv()
        .expect("transaction captured session");
    registry.unregister_preserving_player_state(session_id);
    resume.send(()).expect("resume captured transaction");

    assert!(!transaction_thread.join().expect("transaction thread"));
}
