use super::*;

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use mc_plugin_host::{
    DeploymentConfig, DiscoveryMode, HostQueues, NoSessions, PluginLimits, discover,
    start_deployment,
};
use mc_script::precommit::{
    HookDecision, HookFailure, HookFailurePolicy, HookKind, HookRegistration,
};
use mc_script::{
    ScriptEvent, ScriptHostInput, ScriptPlayerContext, ScriptPlayerId, script_boundary_pair,
};

fn hello_component_bytes() -> Vec<u8> {
    static BYTES: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repository root");
        let sdk = root.join("sdk/rust");
        let status = Command::new(env!("CARGO"))
            .args([
                "build",
                "--manifest-path",
                sdk.join("Cargo.toml")
                    .to_str()
                    .expect("utf-8 workspace path"),
                "--target",
                "wasm32-unknown-unknown",
                "--release",
                "-p",
                "solaris-hello-plugin",
            ])
            .status()
            .expect("guest build starts");
        assert!(status.success(), "hello guest builds");
        let module = std::fs::read(
            sdk.join("target/wasm32-unknown-unknown/release/solaris_hello_plugin.wasm"),
        )
        .expect("guest module exists");
        wit_component::ComponentEncoder::default()
            .module(&module)
            .expect("guest module carries component types")
            .validate(true)
            .encode()
            .expect("guest module encodes as a component")
    });
    BYTES.clone()
}

fn before_build_boundary() -> (mc_script::ScriptBoundary, mc_script::ScriptHostEndpoint) {
    let (boundary, endpoint) = script_boundary_pair(
        NonZeroUsize::new(8).expect("non-zero event queue"),
        NonZeroUsize::new(8).expect("non-zero command queue"),
    );
    boundary
        .set_precommit_hooks(vec![HookRegistration::new(
            "judge",
            HookKind::Build,
            0,
            HookFailurePolicy::Deny,
        )])
        .expect("one valid before-build hook");
    (boundary, endpoint)
}

fn cancel_next_build(endpoint: &mut mc_script::ScriptHostEndpoint) {
    match endpoint.recv_input_blocking() {
        Some(ScriptHostInput::Precommit(request)) => request
            .answer(HookDecision::Cancel)
            .expect("live native precommit request"),
        _ => panic!("expected one before-build request"),
    }
}

fn keep_next_build(endpoint: &mut mc_script::ScriptHostEndpoint) {
    match endpoint.recv_input_blocking() {
        Some(ScriptHostInput::Precommit(request)) => request
            .answer(HookDecision::Keep)
            .expect("live native precommit request"),
        _ => panic!("expected one before-build request"),
    }
}

async fn admitted_protected_zone() -> mc_script::AdmittedScriptCommand {
    let plugins = tempfile::tempdir().expect("temporary deployment root");
    let plugin = plugins.path().join("zone-fence");
    std::fs::create_dir(&plugin).expect("package directory");
    std::fs::write(
        plugin.join("plugin.toml"),
        "id = \"zone-fence\"\nname = \"Zone fence\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\nevents = [\"player.joined\"]\ncapabilities = [\"zones\"]\n",
    )
    .expect("manifest");
    std::fs::write(plugin.join("config.toml"), "mode = \"zones\"\n").expect("config");
    std::fs::write(plugin.join("plugin.wasm"), hello_component_bytes())
        .expect("component artifact");
    let host = start_deployment(
        discover(
            &DeploymentConfig {
                root: plugins.path().to_path_buf(),
                mode: DiscoveryMode::Strict,
                expected: vec!["zone-fence".to_owned()],
                grants: BTreeMap::new(),
                require_grants: false,
                precommit_hooks: Vec::new(),
            },
            &PluginLimits::default(),
        )
        .expect("strict component deployment discovers")
        .into_packages(),
        PluginLimits::default(),
        HostQueues::default(),
        Arc::new(NoSessions),
    )
    .expect("component host starts");
    let boundary = host.boundary().clone();
    boundary
        .try_enqueue_event(ScriptEvent::player_joined_with_context(
            ScriptPlayerId::new(7),
            ScriptPlayerContext::try_new(
                "12345678-1234-5678-1234-567812345678",
                "Fence",
                false,
                0.0,
                64.0,
                0.0,
            )
            .expect("context"),
        ))
        .expect("join event queues");
    let first = boundary
        .recv_command()
        .await
        .expect("unprotected zone command");
    boundary
        .accept_host_command(first)
        .expect("unprotected zone command is admitted");
    let admitted = boundary
        .accept_host_command(
            boundary
                .recv_command()
                .await
                .expect("protected zone command"),
        )
        .expect("protected zone command is admitted");
    host.stop();
    admitted
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_placement_preserves_world_and_inventory() {
    let (storage, support, support_token) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let target_token = storage.block_mutation_token(target).expect("target token");
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "CancelledPlacement");
    let mut inventory = PlayerInventory::empty();
    let held_slot = PlayerInventory::HOTBAR_BASE;
    inventory.slots[held_slot] = ItemStack::new(42, 2);
    let player = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(4);
    let (boundary, mut endpoint) = before_build_boundary();
    handle.install_precommit_boundary(boundary);
    let player_handle = handle.for_session(session);
    let mut request = Box::pin(player_handle.commit_survival_placement(
        test_survival_placement_plan(target, target_token, support, support_token, 42, 2),
    ));
    assert_request_enqueued(request.as_mut(), &handle).await;
    owner.process_tick_with_world(&registry, Some(&world), None, 1);
    cancel_next_build(&mut endpoint);
    assert!(owner.wait_for_command().await);
    owner.process_tick_with_world(&registry, Some(&world), None, 1);
    assert!(matches!(
        request.await,
        Err(SimulationRequestError::Precommit(_))
    ));
    assert_eq!(
        world.lock().await.get_cached_block(target),
        Some(BlockStateId(0))
    );
    assert_eq!(
        player.lock().unwrap().inventory.slots[held_slot],
        ItemStack::new(42, 2)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_owner_break_preserves_block_tool_and_drops() {
    let (storage, target, target_token) = test_block_storage();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "CancelledBreak");
    let held_slot = PlayerInventory::HOTBAR_BASE;
    let mut inventory = PlayerInventory::empty();
    inventory.slots[held_slot] = ItemStack::new(42, 1);
    let player = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(4);
    let (boundary, mut endpoint) = before_build_boundary();
    handle.install_precommit_boundary(boundary);
    let player_handle = handle.for_session(session);
    let mut request = Box::pin(
        player_handle
            .commit_survival_block_break(test_survival_block_break_plan(target, target_token)),
    );

    assert_request_enqueued(request.as_mut(), &handle).await;
    assert!(owner.wait_for_command().await);
    owner.process_tick_with_world(&registry, Some(&world), None, 1);
    cancel_next_build(&mut endpoint);
    assert!(owner.wait_for_command().await);
    owner.process_tick_with_world(&registry, Some(&world), None, 1);

    assert!(matches!(
        request.await,
        Err(SimulationRequestError::Precommit(HookFailure::Cancelled))
    ));
    assert_eq!(
        world.lock().await.get_cached_block(target),
        Some(BlockStateId(1))
    );
    assert_eq!(
        player.lock().unwrap().inventory.slots[held_slot].damage,
        None
    );
    assert_eq!(persisted_item_drop_count(&registry), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_bucket_use_preserves_block_and_inventory() {
    let (storage, support, _) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let target_token = storage
        .block_mutation_token(target)
        .expect("bucket target token");
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "CancelledBucket");
    let held_slot = PlayerInventory::HOTBAR_BASE;
    let mut inventory = PlayerInventory::empty();
    inventory.slots[held_slot] = ItemStack::new(61, 1);
    let player = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(4);
    let (boundary, mut endpoint) = before_build_boundary();
    handle.install_precommit_boundary(boundary);
    let player_handle = handle.for_session(session);
    let mut request =
        Box::pin(player_handle.commit_bucket_use(test_bucket_use_plan(target, target_token)));

    assert_request_enqueued(request.as_mut(), &handle).await;
    assert!(owner.wait_for_command().await);
    owner.process_tick_with_world(&registry, Some(&world), None, 1);
    cancel_next_build(&mut endpoint);
    assert!(owner.wait_for_command().await);
    owner.process_tick_with_world(&registry, Some(&world), None, 1);

    assert!(matches!(
        request.await,
        Err(SimulationRequestError::Precommit(HookFailure::Cancelled))
    ));
    assert_eq!(
        world.lock().await.get_cached_block(target),
        Some(BlockStateId(0))
    );
    assert_eq!(
        player.lock().unwrap().inventory.slots[held_slot],
        ItemStack::new(61, 1)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn changed_zone_snapshot_refuses_kept_placement_without_mutation() {
    let (storage, support, support_token) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let target_token = storage
        .block_mutation_token(target)
        .expect("placement target token");
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let session = register_test_session(&registry, "ZoneFencePlacement");
    let held_slot = PlayerInventory::HOTBAR_BASE;
    let mut inventory = PlayerInventory::empty();
    inventory.slots[held_slot] = ItemStack::new(42, 2);
    let player = register_test_player_state(&registry, session, inventory);
    let (handle, mut owner) = simulation_channel_with_capacity(4);
    let (boundary, mut endpoint) = before_build_boundary();
    handle.install_precommit_boundary(boundary);
    let (zone_boundary, _zone_endpoint) = script_boundary_pair(
        NonZeroUsize::new(8).expect("non-zero zone event queue"),
        NonZeroUsize::new(8).expect("non-zero zone command queue"),
    );
    let zones =
        crate::script::PluginZoneAdapter::new(crate::server::ScriptEventSink::new(zone_boundary));
    let protected_zone = admitted_protected_zone().await;
    let mut plan =
        test_survival_placement_plan(target, target_token, support, support_token, 42, 2);
    plan.zone_fence = Some(zones.capture_protection_fence());
    let player_handle = handle.for_session(session);
    let mut request = Box::pin(player_handle.commit_survival_placement(plan));

    assert_request_enqueued(request.as_mut(), &handle).await;
    assert!(owner.wait_for_command().await);
    owner.process_tick_with_world(&registry, Some(&world), None, 1);
    zones
        .route_admitted(protected_zone)
        .expect("publish changed zone definition");
    assert!(
        !zones
            .protection_snapshot()
            .expect("read published zone protection")
            .ambient_block_mutation_allowed("minecraft:overworld", target)
    );
    keep_next_build(&mut endpoint);
    assert!(owner.wait_for_command().await);
    owner.process_tick_with_world(&registry, Some(&world), None, 1);

    let outcome = request.await;
    assert!(
        matches!(
            outcome,
            Err(SimulationRequestError::Precommit(
                HookFailure::PermissionDenied
            ))
        ),
        "the changed zone must deny the committed placement, got {outcome:?}"
    );
    assert_eq!(
        world.lock().await.get_cached_block(target),
        Some(BlockStateId(0))
    );
    assert_eq!(
        player.lock().unwrap().inventory.slots[held_slot],
        ItemStack::new(42, 2)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_server_owned_edit_cannot_bypass_before_build() {
    let (storage, support, _) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let registry = SessionRegistry::new();
    let (handle, mut owner) = simulation_channel_with_capacity(4);
    let (boundary, mut endpoint) = before_build_boundary();
    let manifest = mc_script::ScriptPluginManifest::new(
        "judge",
        "Judge",
        "0.1.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .validate()
    .expect("valid source plugin");
    endpoint
        .register_plugin_routes(&manifest)
        .expect("live programmatic source registration");
    handle.install_precommit_boundary(boundary);
    let mut request = Box::pin(handle.apply_server_owned_block_edits(
        "judge",
        vec![BlockEdit {
            pos: target,
            new_state: BlockStateId(1),
        }],
        None,
    ));
    assert_request_enqueued(request.as_mut(), &handle).await;

    assert!(owner.wait_for_command().await);
    owner.process_tick_with_world(&registry, Some(&world), None, 1);
    std::future::poll_fn(|context| {
        let result = request.as_mut().poll(context);
        assert!(
            result.is_pending(),
            "request ended before its hook: {result:?}"
        );
        std::task::Poll::Ready(())
    })
    .await;
    cancel_next_build(&mut endpoint);
    assert_request_enqueued(request.as_mut(), &handle).await;
    assert!(owner.wait_for_command().await);
    owner.process_tick_with_world(&registry, Some(&world), None, 1);

    assert!(matches!(
        request.await,
        Err(SimulationRequestError::Precommit(HookFailure::Cancelled))
    ));
    assert_eq!(
        world.lock().await.get_cached_block(target),
        Some(BlockStateId(0))
    );
}

/// Receipt-bearing construction keeps its precommit fence after leaving the
/// canonical server-owned lane: cancellation closes the reserved journal id
/// without changing blocks or retaining a recoverable receipt.
#[tokio::test(flavor = "current_thread")]
async fn cancelled_journaled_structure_portion_keeps_blocks_and_receipt_uncommitted() {
    let (storage, support, _) = test_block_storage();
    let target = BlockPos {
        x: support.x + 1,
        ..support
    };
    let read_view = storage.read_view();
    let mutation_view = storage.mutation_view();
    let world = Arc::new(tokio::sync::Mutex::new(storage));
    let temp = tempfile::tempdir().unwrap();
    let blocks = Arc::new(BlockRegistry::from_report(&test_block_reports()).unwrap());
    let items = Arc::new(mc_data::items::solaris_required_items());
    let (journal, pending) =
        crate::play::world_journal::WorldChunkJournal::open_for_test(temp.path(), blocks, items)
            .unwrap();
    assert!(pending.is_empty());
    let registry = SessionRegistry::new();
    registry.install_world_chunk_journal(journal);
    let (handle, mut owner) = simulation_channel_with_capacity(4);
    let (boundary, mut endpoint) = before_build_boundary();
    let manifest = mc_script::ScriptPluginManifest::new(
        "judge",
        "Judge",
        "0.1.0",
        mc_script::COMPONENT_PLUGIN_API_VERSION,
    )
    .validate()
    .expect("valid source plugin");
    endpoint
        .register_plugin_routes(&manifest)
        .expect("live programmatic source registration");
    handle.install_precommit_boundary(boundary);
    let batch: crate::script::storage::PreparedStorageBatch =
        serde_json::from_value(serde_json::json!({
            "transaction_id": 1,
            "plugin_id": "judge",
            "mutations": [{
                "kind": "compare_and_swap",
                "key": "structure-progress",
                "expected_version": null,
                "value": "1"
            }]
        }))
        .unwrap();
    let resources = crate::chunk_pipeline::ChunkPipelineResources::with_limits(1, 2);
    let mut request = Box::pin(handle.commit_server_owned_block_edits(
        "judge",
        vec![BlockEdit::new(target, BlockStateId(1))],
        None,
        batch.encode_world_decision().unwrap(),
    ));

    // The submitter first reads exact block tokens, then waits for the hook.
    assert_request_enqueued(request.as_mut(), &handle).await;
    assert_eq!(
        owner
            .process_commands_with_world_views(
                &registry,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await
            .processed,
        1
    );
    std::future::poll_fn(|context| {
        assert!(
            request.as_mut().poll(context).is_pending(),
            "receipt portion must wait for before-build"
        );
        std::task::Poll::Ready(())
    })
    .await;
    cancel_next_build(&mut endpoint);
    assert_request_enqueued(request.as_mut(), &handle).await;
    assert_eq!(
        owner
            .process_commands_with_world_views(
                &registry,
                Some(&world),
                SimulationWorldAccess {
                    read: Some(&read_view),
                    mutation: Some(&mutation_view),
                    cpu: Some(&resources),
                    light: None,
                },
                None,
                1,
            )
            .await
            .processed,
        1
    );

    assert!(matches!(
        request.await,
        Err(SimulationRequestError::Precommit(HookFailure::Cancelled))
    ));
    assert_eq!(
        world.lock().await.get_cached_block(target),
        Some(BlockStateId(0))
    );
    let journal = registry.world_chunk_journal().unwrap();
    let pending = journal.pending_decisions_for_test();
    assert_eq!(pending.len(), 1, "the refusal closes its reserved id");
    assert_eq!(pending[0].inventory_batch().unwrap(), None);
    assert!(
        journal.decode_pending(&pending).unwrap().is_empty(),
        "a refused portion records neither after-image nor receipt"
    );
}
