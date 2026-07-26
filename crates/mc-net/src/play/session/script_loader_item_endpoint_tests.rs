use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::task::Poll;

use mc_data::Identifier;
use mc_protocol::packets::play::ItemStack;
use mc_script::ScriptPlayerInventoryFailure;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::loader::{
    LOADER_PROTOCOL_VERSION, LoaderBundle, LoaderClientAck, LoaderContentKind, LoaderManifest,
    LoaderPermission, LoaderPlatform,
};
use crate::login::LoggedInProfile;
use crate::play::PlayerPose;
use crate::play::inventory::PlayerInventory;
use crate::play::persistence::PlayerPersistedState;

use super::outbound::OutboundCommand;
use super::script_loader_item_endpoint::apply_loader_item_grant;
use super::{SessionRegistration, SessionRegistry};

fn loader_manifest() -> LoaderManifest {
    LoaderManifest {
        protocol: LOADER_PROTOCOL_VERSION,
        bundles: vec![LoaderBundle {
            owner: "example".to_owned(),
            id: "block".to_owned(),
            version: "1".to_owned(),
            artifact: "client/block.zip".to_owned(),
            sha256: "a".repeat(64),
            size_bytes: 1,
            loaders: vec![LoaderPlatform::Fabric],
            content: vec![LoaderContentKind::Blocks],
            permissions: vec![LoaderPermission::RegisterBlocks],
            cache_key: format!("example:block/1/{}", "a".repeat(64)),
            source_path: None,
            block_id: Some("example:ruby_block".to_owned()),
            block_name: Some("Ruby Block".to_owned()),
        }],
    }
}

fn loader_session(manifest: &LoaderManifest) -> crate::LoaderSession {
    manifest
        .bind_ack(&LoaderClientAck {
            protocol: LOADER_PROTOCOL_VERSION,
            platform: LoaderPlatform::Fabric,
            loader_version: "test".to_owned(),
            accepted_permissions: manifest.bundles[0].permissions.clone(),
            cached_bundles: vec![manifest.bundles[0].cache_key.clone()],
            carrier_block_state_ids: BTreeMap::from([("example:ruby_block".to_owned(), 321)]),
        })
        .unwrap()
}

fn register_loader_player(
    loader_session: Option<crate::LoaderSession>,
    inventory: PlayerInventory,
) -> (
    SessionRegistry,
    u64,
    Arc<Mutex<PlayerPersistedState>>,
    mpsc::Receiver<OutboundCommand>,
) {
    let registry = SessionRegistry::new();
    let profile = LoggedInProfile {
        uuid: Uuid::from_u128(1),
        name: "LoaderItemTest".to_owned(),
    };
    let (tx, receiver) = mpsc::channel(4);
    let (session_id, _) = registry
        .try_register(SessionRegistration {
            profile: &profile,
            properties: &[],
            center: (0, 0),
            view_distance: 2,
            desired: HashSet::new(),
            tx,
            pose: PlayerPose::new(0.5, 64.0, 0.5),
            max_sessions: usize::MAX,
            script_operator: false,
            dimension: "minecraft:overworld",
            loader_session,
        })
        .unwrap();
    let mut persisted = PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5));
    persisted.inventory = inventory;
    let persisted = Arc::new(Mutex::new(persisted));
    registry.register_player_persistence(session_id, Arc::clone(&persisted));
    (registry, session_id, persisted, receiver)
}

#[tokio::test]
async fn loader_item_grant_waits_for_exact_acknowledged_session_owner_commit() {
    let manifest = loader_manifest();
    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let stack = manifest
        .world_block_item("example", "example:ruby_block", 3, &items)
        .unwrap();
    let mut initial = PlayerInventory::empty();
    let mut existing = stack.clone();
    existing.count = 2;
    initial.slots[9] = existing;
    let (registry, player_id, persisted, mut outbound) =
        register_loader_player(Some(loader_session(&manifest)), initial.clone());
    let mut routed =
        Box::pin(registry.route_loader_item_grant(player_id, "example:ruby_block", stack.clone()));

    std::future::poll_fn(|context| {
        assert!(routed.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;
    assert_eq!(persisted.lock().unwrap().inventory.slots, initial.slots);
    let OutboundCommand::LoaderItemGrant(command) = outbound.recv().await.unwrap() else {
        panic!("expected Loader item grant");
    };
    assert_eq!(command.stack(), &stack);
    let mut owner_inventory = initial;
    let result = {
        let _transaction_guard = command.begin_commit().expect("session remains active");
        let mut saved = persisted.lock().unwrap();
        apply_loader_item_grant(
            command.stack(),
            &mut owner_inventory,
            &mut saved,
            &items,
            &facts,
        )
    };
    command.complete(result);

    assert_eq!(routed.await, Ok(()));
    let mut merged = stack;
    merged.count = 5;
    assert_eq!(owner_inventory.slots[9], merged);
    assert_eq!(
        persisted.lock().unwrap().inventory.slots,
        owner_inventory.slots
    );
}

#[tokio::test]
async fn loader_item_grant_requires_exact_ack_and_preserves_full_inventory() {
    let manifest = loader_manifest();
    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let paper = items
        .id_of(&Identifier::parse("minecraft:paper").unwrap())
        .unwrap();
    let stack = manifest
        .world_block_item("example", "example:ruby_block", 1, &items)
        .unwrap();
    let (registry, player_id, _, _) = register_loader_player(None, PlayerInventory::empty());
    assert_eq!(
        registry
            .route_loader_item_grant(player_id, "example:ruby_block", stack.clone())
            .await,
        Err(ScriptPlayerInventoryFailure::PlayerUnavailable)
    );

    let mut full = PlayerInventory::empty();
    for slot in 9..=44 {
        full.slots[slot] = ItemStack::new(paper, 64);
    }
    let before = full.clone();
    let mut persisted = PlayerPersistedState::new_default(PlayerPose::new(0.5, 64.0, 0.5));
    persisted.inventory = full.clone();
    assert_eq!(
        apply_loader_item_grant(&stack, &mut full, &mut persisted, &items, &facts),
        Err(ScriptPlayerInventoryFailure::InventoryFull)
    );
    assert_eq!(full.slots, before.slots);
    assert_eq!(persisted.inventory.slots, before.slots);
}
