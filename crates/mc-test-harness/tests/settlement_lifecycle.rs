//! Behavioral coverage for the shipped `solaris-settlements` resident-site
//! lifecycle.
//!
//! A resident spawn reserves a durable site POI in the core. The package may
//! only hand that reservation back (through `solaris.release_resident_site`)
//! when the core refused the spawn before it could touch the world, must keep
//! the durable intent and re-query it by operation id for every ambiguous or
//! absent answer, and must never let a stale completion of an older operation
//! disturb the intent a newer operation owns.
//!
//! Each case copies the sibling package into a temp root, strict-loads it,
//! satisfies its `server.started` read, drives `create`/`adopt`/`populate`
//! through the script boundary exactly like `plugin_standard_pack.rs` does,
//! and then injects operation receipts at the two operations under test.

use std::path::Path;
use std::time::Duration;

use mc_script::{
    AdmittedScriptCommand, MAX_SCRIPT_ID_BYTES, PlayerCommandAdmission, ScriptBoundary,
    ScriptChunkAvailability, ScriptCommand, ScriptEvent, ScriptInventoryEndpoint,
    ScriptInventoryFence, ScriptInventoryItem, ScriptInventoryReservationQuantity,
    ScriptInventoryReservationSnapshot, ScriptInventorySlot, ScriptOperation,
    ScriptOperationFailure, ScriptOperationOutcome, ScriptOperationPayload,
    ScriptOwnedInventoryOperation, ScriptOwnedInventoryResult, ScriptOwnedInventorySnapshot,
    ScriptPlayerContext, ScriptPlayerId, ScriptResidentLifecycle, ScriptResidentOperation,
    ScriptResidentOrderOperation, ScriptResidentPois, ScriptResidentResult,
    ScriptResidentSiteReservation, ScriptResidentSnapshot, ScriptResidentWorkOrder,
    ScriptSettlementOperation, ScriptSettlementPoi, ScriptSettlementResult, ScriptSettlementSite,
    ScriptSitePoiKind, ScriptSitePoiState, ScriptSiteProvenance, ScriptSiteVariant,
    ScriptStorageChange, ScriptStorageMutation, ScriptStructureMaterial, ScriptStructureReceipt,
    ScriptStructureSnapshot, ScriptStructureStagePlan, ScriptStructureState, ScriptSurveyBounds,
    ScriptSurveySnapshot, ScriptWarehouseBinding, resident_entity_uuid,
    resident_handle_for_generation, warehouse_handle,
};

const PLUGIN: &str = "solaris-settlements";
const INDEX_KEY: &str = "settlements-index-v1";
const SETTLEMENT: &str = "hamlet";
const OPS_KEY: &str = "ops:hamlet";
const SITE_KEY: &str = "site:hamlet";
const SITE_ID: &str = "site-0001";
const POI_ID: &str = "home-0";
const TOKEN: &str = "spawn-token-1";
const TOKEN_2: &str = "spawn-token-2";
const PLAYER_UUID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const COMMAND_WAIT: Duration = Duration::from_secs(5);
const PLAYER: u64 = 7;

/// The workplace a haul's other half is: core binds its authored container, and
/// the package may only name the handle core mints for that binding.
const WAREHOUSE_BLUEPRINT: &str = "solaris:warehouse";
/// The name the package derives for that project: the blueprint's first eight
/// characters plus the placement ordinal.
const WAREHOUSE_BUILDING: &str = "warehous_1";
const STRUCTURE_ID: &str = "structure-warehouse-1";
/// The reservation core commits when the plan's materials are funded.
const RESERVATION: &str = "reservation-1";
/// The four stages a warehouse is built in, each one work unit.
const WAREHOUSE_STAGES: [&str; 4] = ["foundation", "frame", "roof", "fitting"];
/// The resource every warehouse stage consumes, one unit per stage.
const WAREHOUSE_MATERIAL: &str = "minecraft:stone";
/// Core's canonical hash of the warehouse resource plan.
const PLAN_HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
/// The canonical before-image hash core reports for an inventory fence.
const SNAPSHOT_HASH: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
/// The inventory revision the worker's own equipment was read at; the work
/// order is fenced on it.
const EQUIPMENT_REVISION: u64 = 3;
/// The generation id core mints the spawned resident's identity from.
const RESIDENT_GENERATION: &str = "resident01";

/// One resident spawn that is durably pending in the package.
struct PendingSpawn {
    admitted: AdmittedScriptCommand,
    operation_id: String,
}

/// One resident the package has spawned: the durable name the job command
/// addresses it by, and the stored value a later read of it must answer with.
struct SpawnedResident {
    name: String,
    stored: String,
}

#[tokio::test]
async fn confirmed_pre_effect_refusal_releases_the_reservation_and_clears_the_intent() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let (boundary, host) = start_host(plugins.path()).await;
    let context = player_context();
    let player_id = ScriptPlayerId::new(PLAYER);
    let pending = drive_to_pending_spawn(&boundary, player_id, &context).await;

    // `blocked` is a refusal the core decides before it materialises anything,
    // so the reservation must be handed back through a persisted release intent.
    boundary
        .try_enqueue_event(
            pending
                .admitted
                .operation_result(ScriptOperationOutcome::rejected(
                    ScriptOperationFailure::Blocked,
                ))
                .expect("blocked spawn result"),
        )
        .expect("deliver blocked spawn result");

    let release_intent = next_command(&boundary, "release intent bundle").await;
    assert_eq!(release_intent.plugin_id(), PLUGIN);
    let ops = mutation_value(&release_intent, OPS_KEY)
        .expect("the release intent must persist the operations index");
    assert!(
        ops.contains("release_site") && ops.contains(TOKEN),
        "the durable release intent must name its own operation and the reserved token, saw {ops:?}"
    );
    deliver_storage_batch(&boundary, release_intent, 4).await;

    let release = next_command(&boundary, "release call").await;
    assert_eq!(release.plugin_id(), PLUGIN);
    let released_operation = match operation(&release) {
        ScriptOperation::Settlement {
            operation:
                ScriptSettlementOperation::ReleaseResidentSite {
                    operation_id,
                    spawn_site_token,
                },
        } => {
            assert_eq!(spawn_site_token, TOKEN);
            operation_id.clone()
        }
        other => panic!("expected one release_resident_site call, saw {other:?}"),
    };

    boundary
        .try_enqueue_event(
            release
                .operation_result(resident_site_outcome(1, TOKEN))
                .expect("committed release result"),
        )
        .expect("deliver committed release result");

    let settled = next_command(&boundary, "release commit bundle").await;
    let ops = mutation_value(&settled, OPS_KEY).expect("the cleared index must be persisted");
    assert_eq!(
        ops, "v1",
        "a confirmed release must leave no resident-site intent behind, saw {ops:?}"
    );
    deliver_storage_batch(&boundary, settled, 5).await;
    expect_chat(
        &boundary,
        player_id,
        "The refused spawn left the site reservation free again.",
    )
    .await;
    assert_ne!(released_operation, pending.operation_id);

    stop_host(boundary, host).await;
}

#[tokio::test]
async fn absent_spawn_receipt_keeps_the_reservation_and_requeries_the_operation() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let (boundary, host) = start_host(plugins.path()).await;
    let context = player_context();
    let player_id = ScriptPlayerId::new(PLAYER);
    let pending = drive_to_pending_spawn(&boundary, player_id, &context).await;

    // `not_found` is how the core answers for a receipt it cannot produce; an
    // absent receipt never proves the spawn did not commit, so nothing may be
    // released and the durable intent must stay pending.
    boundary
        .try_enqueue_event(
            pending
                .admitted
                .operation_result(ScriptOperationOutcome::rejected(
                    ScriptOperationFailure::NotFound,
                ))
                .expect("not-found spawn result"),
        )
        .expect("deliver not-found spawn result");

    // Each probe is only issued while the pending intent still exists, so the
    // second and third probe are direct evidence the intent survived.
    let mut probe = Some(next_command(&boundary, "first operation status probe").await);
    expect_chat(&boundary, player_id, "Core refused the request: not_found.").await;
    for attempt in 1..=3 {
        let admitted = probe
            .take()
            .expect("a probe is issued before each delivery");
        match operation(&admitted) {
            ScriptOperation::Status { operation_id } => {
                assert_eq!(
                    operation_id, &pending.operation_id,
                    "probe {attempt} must re-query the pending spawn operation"
                );
            }
            other => panic!("expected an operation_status probe, saw {other:?}"),
        }
        assert!(
            !matches!(operation(&admitted), ScriptOperation::Settlement { .. }),
            "an absent spawn receipt must never release the site reservation"
        );
        boundary
            .try_enqueue_event(
                admitted
                    .operation_result(ScriptOperationOutcome::rejected(
                        ScriptOperationFailure::NotFound,
                    ))
                    .expect("not-found status result"),
            )
            .expect("deliver not-found status result");
        if attempt < 3 {
            probe = Some(next_command(&boundary, "next operation status probe").await);
        }
    }

    stop_host(boundary, host).await;
}

#[tokio::test]
async fn stale_spawn_completion_cannot_touch_a_newer_site_intent() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let (boundary, host) = start_host(plugins.path()).await;
    let context = player_context();
    let player_id = ScriptPlayerId::new(PLAYER);
    let stale = drive_to_pending_spawn(&boundary, player_id, &context).await;

    // A second populate parks a newer reservation intent on the same target
    // while the older spawn request is still unanswered.
    run_player_command(&boundary, player_id, &context, "settlement populate hamlet");
    let reserve_intent = next_command(&boundary, "second reserve intent bundle").await;
    deliver_storage_batch(&boundary, reserve_intent, 4).await;
    let newer = next_command(&boundary, "second reserve call").await;
    let newer_operation = match operation(&newer) {
        ScriptOperation::Settlement {
            operation: ScriptSettlementOperation::ReserveResidentSite { operation_id, .. },
        } => operation_id.clone(),
        other => panic!("expected a second reserve_resident_site call, saw {other:?}"),
    };
    assert_ne!(newer_operation, stale.operation_id);

    // The stale completion must not release the token the newer intent owns.
    boundary
        .try_enqueue_event(
            stale
                .admitted
                .operation_result(ScriptOperationOutcome::rejected(
                    ScriptOperationFailure::Blocked,
                ))
                .expect("stale blocked spawn result"),
        )
        .expect("deliver stale blocked spawn result");
    expect_chat(&boundary, player_id, "Core refused the request: blocked.").await;

    // The newer reservation still resolves, and its completion spawns with the
    // newer token: the older completion neither released nor cleared it.
    boundary
        .try_enqueue_event(
            newer
                .operation_result(resident_site_outcome(1, TOKEN_2))
                .expect("committed newer reservation result"),
        )
        .expect("deliver committed newer reservation result");
    let spawn_intent = next_command(&boundary, "newer spawn intent bundle").await;
    let ops = mutation_value(&spawn_intent, OPS_KEY).expect("spawn intent must persist the index");
    assert!(
        ops.contains("spawn") && ops.contains(TOKEN_2),
        "the newer intent must survive into the spawn intent, saw {ops:?}"
    );
    deliver_storage_batch(&boundary, spawn_intent, 5).await;
    let spawn = next_command(&boundary, "newer spawn call").await;
    match operation(&spawn) {
        ScriptOperation::Resident {
            operation:
                ScriptResidentOperation::Spawn {
                    operation_id,
                    spawn_site_token,
                    ..
                },
        } => {
            assert_eq!(spawn_site_token, TOKEN_2);
            assert!(operation_id.starts_with("spawn-"));
        }
        other => panic!("expected one spawn_resident call, saw {other:?}"),
    }

    stop_host(boundary, host).await;
}

#[tokio::test]
async fn names_differing_only_by_separator_create_distinct_durable_settlements() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let (boundary, host) = start_host(plugins.path()).await;
    let context = player_context();
    let player_id = ScriptPlayerId::new(PLAYER);

    // `a-b` and `a_b` are both accepted names. They may not sanitize onto one
    // durable identity: the core keys the create operation by the batch id the
    // package builds, so aliasing them makes the second create an
    // `operation_conflict` instead of a settlement.
    run_player_command(
        &boundary,
        player_id,
        &context,
        "settlement create a-b small",
    );
    let first = next_command(&boundary, "a-b create bundle").await;
    let first_id = storage_batch_id(&first);
    assert!(
        mutation_keys(&first)
            .iter()
            .any(|key| key == "settlement:a-b"),
        "the first create must own the `a-b` value record, saw {:?}",
        mutation_keys(&first)
    );
    assert!(
        !mutation_keys(&first)
            .iter()
            .any(|key| key == "settlement:a_b"),
        "the `a-b` create must not touch the `a_b` value record"
    );
    deliver_storage_batch(&boundary, first, 1).await;
    expect_chat(&boundary, player_id, "Founded a-b (small hamlet).").await;

    run_player_command(
        &boundary,
        player_id,
        &context,
        "settlement create a_b small",
    );
    let second = next_command(&boundary, "a_b create bundle").await;
    let second_id = storage_batch_id(&second);
    assert_ne!(
        first_id, second_id,
        "names differing only by `-`/`_` must not share one durable create operation"
    );
    for id in [&first_id, &second_id] {
        assert!(
            id.len() <= MAX_SCRIPT_ID_BYTES,
            "a create id must stay inside the core's bounded id contract, saw {id:?} ({} bytes)",
            id.len()
        );
    }
    let index =
        mutation_value(&second, INDEX_KEY).expect("the second create must persist the index");
    assert!(
        index.contains("a-b") && index.contains("a_b"),
        "neither settlement may overwrite the other in the workspace index, saw {index:?}"
    );
    assert!(
        mutation_keys(&second)
            .iter()
            .any(|key| key == "settlement:a_b"),
        "the second create must own its own value record, saw {:?}",
        mutation_keys(&second)
    );
    deliver_storage_batch(&boundary, second, 2).await;
    expect_chat(&boundary, player_id, "Founded a_b (small hamlet).").await;

    stop_host(boundary, host).await;
}

#[tokio::test]
async fn ambiguous_spawn_refusals_keep_the_reservation_while_unloaded_releases_it() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let (boundary, host) = start_host(plugins.path()).await;
    let context = player_context();
    let player_id = ScriptPlayerId::new(PLAYER);
    let pending = drive_to_pending_spawn(&boundary, player_id, &context).await;

    // `runtime_unavailable` also collapses a failure that happened after the
    // owner entity was spawned, so it never proves a non-commit: the token must
    // stay reserved and the durable operation must be re-queried by identity.
    boundary
        .try_enqueue_event(
            pending
                .admitted
                .operation_result(ScriptOperationOutcome::rejected(
                    ScriptOperationFailure::RuntimeUnavailable,
                ))
                .expect("runtime_unavailable spawn result"),
        )
        .expect("deliver runtime_unavailable spawn result");
    let probe = next_command(&boundary, "runtime_unavailable status probe").await;
    match operation(&probe) {
        ScriptOperation::Status { operation_id } => assert_eq!(
            operation_id, &pending.operation_id,
            "an ambiguous spawn answer must re-query the pending spawn operation"
        ),
        other => panic!("expected the pending spawn operation to be re-queried, saw {other:?}"),
    }
    expect_chat(
        &boundary,
        player_id,
        "Core settlement runtime is not installed in this build; nothing changed.",
    )
    .await;

    // An absent receipt is indistinguishable from a synthesized one, so it
    // keeps the reservation exactly like `runtime_unavailable` does.
    boundary
        .try_enqueue_event(
            probe
                .operation_result(ScriptOperationOutcome::rejected(
                    ScriptOperationFailure::NotFound,
                ))
                .expect("not-found status result"),
        )
        .expect("deliver not-found status result");
    let second_probe = next_command(&boundary, "absent-receipt status probe").await;
    match operation(&second_probe) {
        ScriptOperation::Status { operation_id } => assert_eq!(
            operation_id, &pending.operation_id,
            "an absent spawn receipt must keep re-querying the same operation"
        ),
        other => {
            panic!("an absent spawn receipt must never release the reservation, saw {other:?}")
        }
    }

    // `unloaded` is decided before any entity is materialised, so a receipt
    // proving it releases the reservation through the same durable intent path
    // and then clears it.
    boundary
        .try_enqueue_event(
            second_probe
                .operation_result(ScriptOperationOutcome::rejected(
                    ScriptOperationFailure::Unloaded,
                ))
                .expect("unloaded status result"),
        )
        .expect("deliver unloaded status result");
    let release_intent = next_command(&boundary, "release intent bundle").await;
    let ops = mutation_value(&release_intent, OPS_KEY)
        .expect("the release intent must persist the index");
    assert!(
        ops.contains("release_site") && ops.contains(TOKEN),
        "the durable release intent must name its own operation and the reserved token, saw {ops:?}"
    );
    deliver_storage_batch(&boundary, release_intent, 5).await;

    let release = next_command(&boundary, "release call").await;
    match operation(&release) {
        ScriptOperation::Settlement {
            operation:
                ScriptSettlementOperation::ReleaseResidentSite {
                    spawn_site_token, ..
                },
        } => assert_eq!(spawn_site_token, TOKEN),
        other => panic!("expected one release_resident_site call, saw {other:?}"),
    }
    boundary
        .try_enqueue_event(
            release
                .operation_result(resident_site_outcome(1, TOKEN))
                .expect("committed release result"),
        )
        .expect("deliver committed release result");

    let settled = next_command(&boundary, "release commit bundle").await;
    let ops = mutation_value(&settled, OPS_KEY).expect("the cleared index must be persisted");
    assert_eq!(
        ops, "v1",
        "a confirmed release must leave no resident-site intent behind, saw {ops:?}"
    );
    deliver_storage_batch(&boundary, settled, 6).await;
    expect_chat(
        &boundary,
        player_id,
        "The refused spawn left the site reservation free again.",
    )
    .await;

    stop_host(boundary, host).await;
}

#[tokio::test]
async fn an_absent_release_receipt_reissues_the_hand_back_instead_of_forgetting_it() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let (boundary, host) = start_host(plugins.path()).await;
    let context = player_context();
    let player_id = ScriptPlayerId::new(PLAYER);
    let pending = drive_to_pending_spawn(&boundary, player_id, &context).await;

    // `unloaded` is decided before any entity is materialised, so the spawn is
    // refused before it could take effect and the reservation must be handed
    // back through a persisted release intent.
    boundary
        .try_enqueue_event(
            pending
                .admitted
                .operation_result(ScriptOperationOutcome::rejected(
                    ScriptOperationFailure::Unloaded,
                ))
                .expect("unloaded spawn result"),
        )
        .expect("deliver unloaded spawn result");
    let release_intent = next_command(&boundary, "release intent bundle").await;
    let ops = mutation_value(&release_intent, OPS_KEY)
        .expect("the release intent must persist the index");
    assert!(
        ops.contains("release_site") && ops.contains(TOKEN),
        "the durable release intent must name its own operation and the reserved token, saw {ops:?}"
    );
    deliver_storage_batch(&boundary, release_intent, 5).await;

    let release = next_command(&boundary, "release call").await;
    let release_operation = match operation(&release) {
        ScriptOperation::Settlement {
            operation:
                ScriptSettlementOperation::ReleaseResidentSite {
                    operation_id,
                    spawn_site_token,
                },
        } => {
            assert_eq!(spawn_site_token, TOKEN);
            operation_id.clone()
        }
        other => panic!("expected one release_resident_site call, saw {other:?}"),
    };
    assert_ne!(release_operation, pending.operation_id);

    // A rejection that is not a definite ledger answer proves nothing about the
    // hand-back, so the package probes the release operation instead of
    // clearing the intent.
    boundary
        .try_enqueue_event(
            release
                .operation_result(ScriptOperationOutcome::rejected(
                    ScriptOperationFailure::RuntimeUnavailable,
                ))
                .expect("runtime_unavailable release result"),
        )
        .expect("deliver runtime_unavailable release result");
    let probe = next_command(&boundary, "release status probe").await;
    match operation(&probe) {
        ScriptOperation::Status { operation_id } => assert_eq!(
            operation_id, &release_operation,
            "an ambiguous release answer must re-query the release operation"
        ),
        other => panic!("expected the release operation to be re-queried, saw {other:?}"),
    }
    expect_chat(
        &boundary,
        player_id,
        "Core settlement runtime is not installed in this build; nothing changed.",
    )
    .await;

    // An absent receipt never becomes present by re-querying it: the hand-back
    // is reissued under the SAME durable operation id and token, not forgotten
    // and not replaced by another status probe.
    boundary
        .try_enqueue_event(
            probe
                .operation_result(ScriptOperationOutcome::rejected(
                    ScriptOperationFailure::NotFound,
                ))
                .expect("not-found release status result"),
        )
        .expect("deliver not-found release status result");
    let retry = next_command(&boundary, "reissued release call").await;
    match operation(&retry) {
        ScriptOperation::Settlement {
            operation:
                ScriptSettlementOperation::ReleaseResidentSite {
                    operation_id,
                    spawn_site_token,
                },
        } => {
            assert_eq!(
                operation_id, &release_operation,
                "the hand-back must be reissued under its own durable operation id"
            );
            assert_eq!(spawn_site_token, TOKEN);
        }
        other => panic!(
            "an absent release receipt must reissue the hand-back, saw {other:?} instead of an operation status probe"
        ),
    }

    // The intent is still the persisted authority: the next bundle re-encodes
    // the operations index with the release intent still in it.
    run_player_command(
        &boundary,
        player_id,
        &context,
        "settlement role hamlet bbbbbbbb-cccc-dddd-eeee-ffffffffffff steward",
    );
    let roles = next_command(&boundary, "role write bundle").await;
    let ops =
        mutation_value(&roles, OPS_KEY).expect("the role write must persist the operations index");
    assert!(
        ops.contains("release_site") && ops.contains(TOKEN),
        "the absent release receipt must keep the release intent, saw {ops:?}"
    );
    deliver_storage_batch(&boundary, roles, 6).await;
    expect_chat(&boundary, player_id, "Roles updated for hamlet.").await;

    // The reissued call is a real hand-back: committing it must clear the
    // intent exactly like the live (non-recovery) release path does.
    boundary
        .try_enqueue_event(
            retry
                .operation_result(resident_site_outcome(1, TOKEN))
                .expect("committed reissued release result"),
        )
        .expect("deliver committed reissued release result");
    let settled = next_command(&boundary, "reissued release commit bundle").await;
    let ops = mutation_value(&settled, OPS_KEY).expect("the cleared index must be persisted");
    assert_eq!(
        ops, "v1",
        "a committed reissued release must leave no resident-site intent behind, saw {ops:?}"
    );
    deliver_storage_batch(&boundary, settled, 7).await;
    expect_chat(
        &boundary,
        player_id,
        "The refused spawn left the site reservation free again.",
    )
    .await;

    stop_host(boundary, host).await;
}

#[tokio::test]
async fn refused_project_withdrawal_is_confirmed_and_a_failed_cleanup_stays_recoverable() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let (boundary, host) = start_host(plugins.path()).await;
    let context = player_context();
    let player_id = ScriptPlayerId::new(PLAYER);
    drive_to_projected_prepare(&boundary, player_id, &context).await;
    let building_key = "building:hamlet:house_sm_1";
    let building_index_key = "bidx:hamlet";

    let prepare = next_command(&boundary, "prepare call").await;
    let prepare_operation = match operation(&prepare) {
        ScriptOperation::Settlement {
            operation:
                ScriptSettlementOperation::PrepareStructure {
                    operation_id,
                    blueprint_id,
                    ..
                },
        } => {
            assert_eq!(blueprint_id, "solaris:house_small");
            operation_id.clone()
        }
        other => panic!("expected one prepare_structure call, saw {other:?}"),
    };

    // A refused prepare must withdraw the persisted `projected` record through
    // one compensating batch that deletes the value record and drops its index
    // entry; the record is only gone from memory once that batch commits.
    boundary
        .try_enqueue_event(
            prepare
                .operation_result(ScriptOperationOutcome::rejected(
                    ScriptOperationFailure::Blocked,
                ))
                .expect("blocked prepare result"),
        )
        .expect("deliver blocked prepare result");
    let discard = next_command(&boundary, "withdrawal batch").await;
    assert!(
        mutation_deleted(&discard, building_key),
        "the compensating batch must delete the projected value record, saw {:?}",
        mutation_keys(&discard)
    );
    let index =
        mutation_value(&discard, building_index_key).expect("the batch must rewrite the index");
    assert!(
        !index.contains("house_sm_1"),
        "the refused entry must leave the durable building index, saw {index:?}"
    );
    let ops =
        mutation_value(&discard, OPS_KEY).expect("the withdrawal intent must persist the index");
    assert!(
        ops.contains("discard") && ops.contains("house_sm_1"),
        "the withdrawal intent must stay pending until its batch commits, saw {ops:?}"
    );

    // The batch did not land, so nothing durable changed: the prepare intent is
    // put back and re-queried by its own durable operation id.
    boundary
        .try_enqueue_event(
            discard
                .operation_result(ScriptOperationOutcome::rejected(
                    ScriptOperationFailure::StaleRevision,
                ))
                .expect("rejected withdrawal result"),
        )
        .expect("deliver rejected withdrawal result");
    let probe = next_command(&boundary, "failed-withdrawal status probe").await;
    match operation(&probe) {
        ScriptOperation::Status { operation_id } => assert_eq!(
            operation_id, &prepare_operation,
            "a failed withdrawal must re-query the prepare operation it was withdrawing"
        ),
        other => panic!("expected the prepare operation to be re-queried, saw {other:?}"),
    }
    expect_chat(
        &boundary,
        player_id,
        "Storage did not confirm the withdrawal; the refused project stays pending and is re-queried.",
    )
    .await;

    // Recovering that probe re-issues the same prepare operation, never a new
    // project.
    boundary
        .try_enqueue_event(
            probe
                .operation_result(ScriptOperationOutcome::rejected(
                    ScriptOperationFailure::NotFound,
                ))
                .expect("not-found status result"),
        )
        .expect("deliver not-found status result");
    let retry = next_command(&boundary, "recovered prepare call").await;
    match operation(&retry) {
        ScriptOperation::Settlement {
            operation: ScriptSettlementOperation::PrepareStructure { operation_id, .. },
        } => assert_eq!(
            operation_id, &prepare_operation,
            "the withdrawn project must be retried by its durable operation id"
        ),
        other => panic!("expected the prepare operation to be re-issued, saw {other:?}"),
    }

    // The retry is refused too; this withdrawal commits, so the confirm bundle
    // carries no pending intent and no building index entry, and the settlement
    // reports no buildings at all.
    boundary
        .try_enqueue_event(
            retry
                .operation_result(ScriptOperationOutcome::rejected(
                    ScriptOperationFailure::Blocked,
                ))
                .expect("blocked retried prepare result"),
        )
        .expect("deliver blocked retried prepare result");
    let committed = next_command(&boundary, "committed withdrawal batch").await;
    assert!(
        mutation_deleted(&committed, building_key),
        "the retried withdrawal must delete the value record"
    );
    deliver_storage_batch(&boundary, committed, 6).await;

    let confirm = next_command(&boundary, "withdrawal confirm bundle").await;
    assert_eq!(
        mutation_value(&confirm, OPS_KEY).as_deref(),
        Some("v1"),
        "a confirmed withdrawal must leave no pending intent"
    );
    assert_eq!(
        mutation_value(&confirm, building_index_key).as_deref(),
        Some("v1"),
        "a confirmed withdrawal must leave no projected index entry"
    );
    deliver_storage_batch(&boundary, confirm, 7).await;
    expect_chat(
        &boundary,
        player_id,
        "Withdrawn the refused project house_sm_1; it is no longer projected.",
    )
    .await;

    run_player_command(
        &boundary,
        player_id,
        &context,
        "settlement buildings hamlet",
    );
    expect_chat(
        &boundary,
        player_id,
        "No buildings yet; use site/adopt/survey/project.",
    )
    .await;

    stop_host(boundary, host).await;
}

#[tokio::test]
async fn hauling_work_names_the_bound_warehouse_container() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let (boundary, host) = start_host(plugins.path()).await;
    let context = player_context();
    let player_id = ScriptPlayerId::new(PLAYER);
    let (_, structure_id) = drive_to_committed_warehouse(&boundary, player_id, &context).await;
    let resident = drive_to_resident(&boundary, player_id, &context).await;
    let resident_handle =
        resident_handle_for_generation(PLUGIN, RESIDENT_GENERATION).expect("core resident handle");

    // A haul's destination is a container of the settlement's own warehouse, so
    // the bind is admitted before any work order exists: the destination handle
    // can only be the one core mints in this binding.
    let bind = drive_to_haul_bind(&boundary, player_id, &context, &structure_id, &resident).await;
    let bound_handle = warehouse_handle(PLUGIN, &structure_id, 0).expect("core warehouse handle");
    boundary
        .try_enqueue_event(
            bind.operation_result(settlement_outcome(
                18,
                ScriptSettlementResult::Warehouse {
                    binding: Box::new(ScriptWarehouseBinding::new(
                        bound_handle.clone(),
                        structure_id.clone(),
                        0,
                        2,
                    )),
                },
            ))
            .expect("warehouse binding result"),
        )
        .expect("deliver warehouse binding result");

    // The work order is fenced on the worker's own canonical revision, so the
    // equipment is read back before the assignment is admitted.
    let equipment = next_command(&boundary, "work revision read").await;
    match operation(&equipment) {
        ScriptOperation::Inventory {
            operation:
                ScriptOwnedInventoryOperation::Query {
                    endpoint: ScriptInventoryEndpoint::ResidentEquipment { handle },
                    ..
                },
        } => assert_eq!(
            handle, &resident_handle,
            "the work revision must come from the worker's own equipment"
        ),
        other => panic!("expected one resident_equipment read, saw {other:?}"),
    }
    boundary
        .try_enqueue_event(
            equipment
                .operation_result(owned_inventory_outcome(
                    19,
                    ScriptOwnedInventoryResult::Snapshot {
                        inventory: resident_equipment_fixture(&resident_handle, EQUIPMENT_REVISION),
                    },
                ))
                .expect("resident equipment snapshot"),
        )
        .expect("deliver resident equipment snapshot");

    let intent = next_command(&boundary, "work intent bundle").await;
    deliver_storage_batch(&boundary, intent, 20).await;

    let assign = next_command(&boundary, "assign work call").await;
    match operation(&assign) {
        ScriptOperation::ResidentOrder {
            operation:
                ScriptResidentOrderOperation::AssignWork {
                    handle,
                    work,
                    expected_revision,
                    ..
                },
        } => {
            assert_eq!(
                handle, &resident_handle,
                "the assignment must address the resident that was asked for the work"
            );
            assert_eq!(
                *expected_revision, EQUIPMENT_REVISION,
                "the assignment must be fenced on the revision the package just read"
            );
            assert_eq!(
                *work,
                ScriptResidentWorkOrder::Haul {
                    source: ScriptInventoryEndpoint::ResidentCarry {
                        handle: resident_handle.clone(),
                    },
                    destination: ScriptInventoryEndpoint::Warehouse {
                        handle: bound_handle.clone(),
                    },
                    item: None,
                },
                "a haul must load the warehouse container core bound for this settlement, never \
                 the worker's own equipment"
            );
        }
        other => panic!("expected one assign_resident_work call, saw {other:?}"),
    }

    stop_host(boundary, host).await;
}

#[tokio::test]
async fn refused_haul_bind_assigns_no_work_and_reports_the_refusal() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let (boundary, host) = start_host(plugins.path()).await;
    let context = player_context();
    let player_id = ScriptPlayerId::new(PLAYER);
    let (_, structure_id) = drive_to_committed_warehouse(&boundary, player_id, &context).await;
    let resident = drive_to_resident(&boundary, player_id, &context).await;
    let bind = drive_to_haul_bind(&boundary, player_id, &context, &structure_id, &resident).await;

    // `unloaded` is core's answer when no structure runtime can serve the bind,
    // so no container exists to name: the assignment is abandoned rather than
    // sent against a guessed destination.
    boundary
        .try_enqueue_event(
            bind.operation_result(ScriptOperationOutcome::rejected(
                ScriptOperationFailure::Unloaded,
            ))
            .expect("unloaded bind result"),
        )
        .expect("deliver unloaded bind result");
    expect_chat(
        &boundary,
        player_id,
        "No warehouse container is bound: core refused the bind (unloaded); nothing was assigned.",
    )
    .await;

    // A refused bind leaves nothing queued: the next command the boundary
    // admits is the reply to a fresh command, so no work order can be pending
    // behind it. The warehouse itself is untouched by the refusal.
    run_player_command(
        &boundary,
        player_id,
        &context,
        &format!("settlement buildings {SETTLEMENT}"),
    );
    expect_chat(
        &boundary,
        player_id,
        &format!("{WAREHOUSE_BUILDING} {WAREHOUSE_BLUEPRINT} committed"),
    )
    .await;

    stop_host(boundary, host).await;
}

/// (CP-003) An issue command really takes the item it names out of the
/// settlement's own warehouse container and into the worker's equipment: the
/// order names the bound handle as the source, the worker's own endpoint as the
/// destination, the item the caller asked for, and the batch size the caller
/// stated.
#[tokio::test]
async fn issue_work_takes_the_named_item_out_of_the_bound_warehouse() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let (boundary, host) = start_host(plugins.path()).await;
    let context = player_context();
    let player_id = ScriptPlayerId::new(PLAYER);
    let (_, structure_id) = drive_to_committed_warehouse(&boundary, player_id, &context).await;
    let resident = drive_to_resident(&boundary, player_id, &context).await;
    let resident_handle =
        resident_handle_for_generation(PLUGIN, RESIDENT_GENERATION).expect("core resident handle");

    let bind = drive_to_issue_bind(
        &boundary,
        player_id,
        &context,
        &resident,
        "minecraft:iron_hoe",
        2,
        "equipment",
    )
    .await;
    let bound_handle = warehouse_handle(PLUGIN, &structure_id, 0).expect("core warehouse handle");
    match operation(&bind) {
        ScriptOperation::Settlement {
            operation:
                ScriptSettlementOperation::BindWarehouse {
                    structure_id: bound,
                    container_id,
                    ..
                },
        } => {
            assert_eq!(
                bound, &structure_id,
                "the issue must bind the container of the settlement's own committed warehouse"
            );
            assert_eq!(
                *container_id, 0,
                "the authored container ordinal is the package's only choice"
            );
        }
        other => panic!("expected one bind_warehouse call before any work order, saw {other:?}"),
    }
    boundary
        .try_enqueue_event(
            bind.operation_result(settlement_outcome(
                21,
                ScriptSettlementResult::Warehouse {
                    binding: Box::new(ScriptWarehouseBinding::new(
                        bound_handle.clone(),
                        structure_id.clone(),
                        0,
                        2,
                    )),
                },
            ))
            .expect("warehouse binding result"),
        )
        .expect("deliver warehouse binding result");

    // The withdrawal is fenced on the worker's own canonical revision, so the
    // endpoint the items land in is read back before the assignment.
    let revision_read = next_command(&boundary, "issue endpoint read").await;
    match operation(&revision_read) {
        ScriptOperation::Inventory {
            operation:
                ScriptOwnedInventoryOperation::Query {
                    endpoint: ScriptInventoryEndpoint::ResidentEquipment { handle },
                    ..
                },
        } => assert_eq!(
            handle, &resident_handle,
            "an issue into equipment must read the worker's own equipment"
        ),
        other => panic!("expected one resident_equipment read, saw {other:?}"),
    }
    boundary
        .try_enqueue_event(
            revision_read
                .operation_result(owned_inventory_outcome(
                    22,
                    ScriptOwnedInventoryResult::Snapshot {
                        inventory: resident_equipment_fixture(&resident_handle, EQUIPMENT_REVISION),
                    },
                ))
                .expect("resident equipment snapshot"),
        )
        .expect("deliver resident equipment snapshot");

    let intent = next_command(&boundary, "issue intent bundle").await;
    deliver_storage_batch(&boundary, intent, 23).await;

    let assign = next_command(&boundary, "issue assign work call").await;
    match operation(&assign) {
        ScriptOperation::ResidentOrder {
            operation:
                ScriptResidentOrderOperation::AssignWork {
                    handle,
                    work,
                    work_units,
                    expected_revision,
                    ..
                },
        } => {
            assert_eq!(
                handle, &resident_handle,
                "the issue must address the worker asked for"
            );
            assert_eq!(
                *work_units, 2,
                "the batch size is the count the caller stated, never a package default"
            );
            assert_eq!(
                *expected_revision, EQUIPMENT_REVISION,
                "the assignment must be fenced on the revision the package just read"
            );
            assert_eq!(
                *work,
                ScriptResidentWorkOrder::Haul {
                    source: ScriptInventoryEndpoint::Warehouse {
                        handle: bound_handle.clone(),
                    },
                    destination: ScriptInventoryEndpoint::ResidentEquipment {
                        handle: resident_handle.clone(),
                    },
                    item: Some("minecraft:iron_hoe".to_owned()),
                },
                "an issue must take the named item out of the bound container into the worker's \
                 own equipment"
            );
        }
        other => panic!("expected one assign_resident_work call, saw {other:?}"),
    }

    stop_host(boundary, host).await;
}

/// (CP-003) The same command can fill the worker's carried slots: the endpoint
/// the caller states decides both what core reads back and where the order puts
/// the items.
#[tokio::test]
async fn issue_work_can_fill_the_workers_carry() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let (boundary, host) = start_host(plugins.path()).await;
    let context = player_context();
    let player_id = ScriptPlayerId::new(PLAYER);
    let (_, structure_id) = drive_to_committed_warehouse(&boundary, player_id, &context).await;
    let resident = drive_to_resident(&boundary, player_id, &context).await;
    let resident_handle =
        resident_handle_for_generation(PLUGIN, RESIDENT_GENERATION).expect("core resident handle");

    let bind = drive_to_issue_bind(
        &boundary,
        player_id,
        &context,
        &resident,
        "minecraft:stone",
        3,
        "carry",
    )
    .await;
    let bound_handle = warehouse_handle(PLUGIN, &structure_id, 0).expect("core warehouse handle");
    boundary
        .try_enqueue_event(
            bind.operation_result(settlement_outcome(
                24,
                ScriptSettlementResult::Warehouse {
                    binding: Box::new(ScriptWarehouseBinding::new(
                        bound_handle.clone(),
                        structure_id.clone(),
                        0,
                        2,
                    )),
                },
            ))
            .expect("warehouse binding result"),
        )
        .expect("deliver warehouse binding result");

    let revision_read = next_command(&boundary, "issue carry read").await;
    match operation(&revision_read) {
        ScriptOperation::Inventory {
            operation:
                ScriptOwnedInventoryOperation::Query {
                    endpoint: ScriptInventoryEndpoint::ResidentCarry { handle },
                    ..
                },
        } => assert_eq!(
            handle, &resident_handle,
            "an issue into carry must read the worker's own carry"
        ),
        other => panic!("expected one resident_carry read, saw {other:?}"),
    }
    boundary
        .try_enqueue_event(
            revision_read
                .operation_result(owned_inventory_outcome(
                    25,
                    ScriptOwnedInventoryResult::Snapshot {
                        inventory: resident_equipment_fixture(&resident_handle, EQUIPMENT_REVISION),
                    },
                ))
                .expect("resident carry snapshot"),
        )
        .expect("deliver resident carry snapshot");

    let intent = next_command(&boundary, "issue carry intent bundle").await;
    deliver_storage_batch(&boundary, intent, 26).await;

    let assign = next_command(&boundary, "issue carry assign work call").await;
    match operation(&assign) {
        ScriptOperation::ResidentOrder {
            operation: ScriptResidentOrderOperation::AssignWork { work, .. },
        } => assert_eq!(
            *work,
            ScriptResidentWorkOrder::Haul {
                source: ScriptInventoryEndpoint::Warehouse {
                    handle: bound_handle.clone(),
                },
                destination: ScriptInventoryEndpoint::ResidentCarry {
                    handle: resident_handle.clone(),
                },
                item: Some("minecraft:stone".to_owned()),
            },
            "the stated endpoint decides where the issued items land"
        ),
        other => panic!("expected one assign_resident_work call, saw {other:?}"),
    }

    stop_host(boundary, host).await;
}

/// (CP-003) An issue refuses a resident that cannot hold or return the stock,
/// before a container is even bound: a dead worker would strand the items, and a
/// resident without a core handle has no endpoint core could credit.
#[tokio::test]
async fn issue_refuses_a_resident_without_a_core_handle_or_a_life() {
    for (index, value, expected) in [
        (
            2,
            "-",
            "That resident has no core handle; claim or populate it first.",
        ),
        (
            13,
            "dead",
            "That resident is not alive; cannot issue stock.",
        ),
    ] {
        let plugins = tempfile::tempdir().expect("plugin tempdir");
        let (boundary, host) = start_host(plugins.path()).await;
        let context = player_context();
        let player_id = ScriptPlayerId::new(PLAYER);
        let (_, _) = drive_to_committed_warehouse(&boundary, player_id, &context).await;
        let resident = drive_to_resident(&boundary, player_id, &context).await;
        let stored = stored_resident_field(&resident.stored, index, value);

        drive_issue_with_stored_resident(
            &boundary,
            player_id,
            &context,
            &resident,
            &stored,
            "minecraft:iron_hoe",
            1,
        )
        .await;
        expect_chat(&boundary, player_id, expected).await;

        // Nothing was bound and nothing was assigned: the next command the
        // boundary admits is the reply to a fresh one.
        run_player_command(
            &boundary,
            player_id,
            &context,
            &format!("settlement buildings {SETTLEMENT}"),
        );
        expect_chat(
            &boundary,
            player_id,
            &format!("{WAREHOUSE_BUILDING} {WAREHOUSE_BLUEPRINT} committed"),
        )
        .await;

        stop_host(boundary, host).await;
    }
}

/// (CP-003) A refused bind assigns nothing: an issue never reaches a work order
/// with a guessed warehouse handle, and the refusal is reported as its own
/// reason.
#[tokio::test]
async fn refused_issue_bind_assigns_no_work_and_reports_the_refusal() {
    let plugins = tempfile::tempdir().expect("plugin tempdir");
    let (boundary, host) = start_host(plugins.path()).await;
    let context = player_context();
    let player_id = ScriptPlayerId::new(PLAYER);
    let (_, _) = drive_to_committed_warehouse(&boundary, player_id, &context).await;
    let resident = drive_to_resident(&boundary, player_id, &context).await;

    let bind = drive_to_issue_bind(
        &boundary,
        player_id,
        &context,
        &resident,
        "minecraft:iron_hoe",
        1,
        "equipment",
    )
    .await;
    boundary
        .try_enqueue_event(
            bind.operation_result(ScriptOperationOutcome::rejected(
                ScriptOperationFailure::Unloaded,
            ))
            .expect("unloaded bind result"),
        )
        .expect("deliver unloaded bind result");
    expect_chat(
        &boundary,
        player_id,
        "No warehouse container is bound: core refused the bind (unloaded); nothing was assigned.",
    )
    .await;

    // Nothing is left queued behind the refusal: the next admitted command is
    // the reply to a fresh one.
    run_player_command(
        &boundary,
        player_id,
        &context,
        &format!("settlement residents {SETTLEMENT}"),
    );
    expect_chat(
        &boundary,
        player_id,
        &format!(
            "{} family=unassigned job=- service=civilian role=- squad=- life=alive_unloaded gear=-",
            resident.name
        ),
    )
    .await;

    stop_host(boundary, host).await;
}

/// Issue one named item into a worker's own endpoint and stop at the admitted
/// bind: an issue command takes stock out of the settlement's committed
/// warehouse container, so the package binds that container before it builds
/// any work order.
async fn drive_to_issue_bind(
    boundary: &ScriptBoundary,
    player_id: ScriptPlayerId,
    context: &ScriptPlayerContext,
    resident: &SpawnedResident,
    item: &str,
    count: u64,
    endpoint: &str,
) -> AdmittedScriptCommand {
    run_player_command(
        boundary,
        player_id,
        context,
        &format!(
            "settlement issue {SETTLEMENT} {} {item} {count} {endpoint}",
            resident.name
        ),
    );

    let read = next_command(boundary, "issue resident read").await;
    assert!(
        matches!(
            read.request(),
            ScriptCommand::PluginStorageGet { request }
                if request.key() == format!("resident:{SETTLEMENT}:{}", resident.name)
        ),
        "the issue command must reload the resident it issues to, saw {:?}",
        read.request()
    );
    boundary
        .try_enqueue_event(
            read.plugin_storage_get_result(Some(&resident.stored), Some(1))
                .expect("stored resident result"),
        )
        .expect("deliver stored resident");

    next_command(boundary, "issue warehouse bind call").await
}

/// Drive one issue command whose stored resident is rewritten first: the value
/// the package reads back is the only authority on the resident's life and core
/// handle, so a rewritten field is exactly the state a stale or dead resident is
/// in.
async fn drive_issue_with_stored_resident(
    boundary: &ScriptBoundary,
    player_id: ScriptPlayerId,
    context: &ScriptPlayerContext,
    resident: &SpawnedResident,
    stored: &str,
    item: &str,
    count: u64,
) {
    run_player_command(
        boundary,
        player_id,
        context,
        &format!(
            "settlement issue {SETTLEMENT} {} {item} {count} equipment",
            resident.name
        ),
    );
    let read = next_command(boundary, "issue resident read").await;
    assert!(
        matches!(
            read.request(),
            ScriptCommand::PluginStorageGet { request }
                if request.key() == format!("resident:{SETTLEMENT}:{}", resident.name)
        ),
        "the issue command must reload the resident before it binds anything, saw {:?}",
        read.request()
    );
    boundary
        .try_enqueue_event(
            read.plugin_storage_get_result(Some(stored), Some(1))
                .expect("stored resident result"),
        )
        .expect("deliver stored resident");
}

/// One pipe-separated field of a stored resident value replaced, so a case can
/// read back a resident the package must refuse.
fn stored_resident_field(stored: &str, index: usize, value: &str) -> String {
    let mut parts = stored.split('|').collect::<Vec<_>>();
    assert!(index < parts.len(), "stored resident has no field {index}");
    parts[index] = value;
    parts.join("|")
}

/// Drive `create`/`adopt`/`survey`/`project` until the package has one
/// `prepare_structure` call admitted for a persisted `projected` building.
async fn drive_to_projected_prepare(
    boundary: &ScriptBoundary,
    player_id: ScriptPlayerId,
    context: &ScriptPlayerContext,
) {
    run_player_command(
        boundary,
        player_id,
        context,
        "settlement create hamlet small",
    );
    let create = next_command(boundary, "create bundle").await;
    deliver_storage_batch(boundary, create, 1).await;
    expect_chat(boundary, player_id, "Founded hamlet (small hamlet).").await;

    run_player_command(
        boundary,
        player_id,
        context,
        &format!("settlement adopt {SETTLEMENT} {SITE_ID}"),
    );
    let query = next_command(boundary, "site query").await;
    match operation(&query) {
        ScriptOperation::Settlement {
            operation: ScriptSettlementOperation::QuerySite { site_id, .. },
        } => assert_eq!(site_id, SITE_ID),
        other => panic!("expected one query_settlement_site call, saw {other:?}"),
    }
    boundary
        .try_enqueue_event(
            query
                .operation_result(settlement_outcome(
                    1,
                    ScriptSettlementResult::Site {
                        site: Box::new(site_fixture()),
                    },
                ))
                .expect("site snapshot result"),
        )
        .expect("deliver site snapshot result");
    let adopt = next_command(boundary, "site write bundle").await;
    deliver_storage_batch(boundary, adopt, 2).await;
    expect_chat(
        boundary,
        player_id,
        "Adopted site-0001 (hamlet): 0 buildings, 1 points of interest, revision 1.",
    )
    .await;

    run_player_command(
        boundary,
        player_id,
        context,
        "settlement survey hamlet plot",
    );
    let survey_intent = next_command(boundary, "survey intent bundle").await;
    deliver_storage_batch(boundary, survey_intent, 3).await;
    let survey = next_command(boundary, "survey call").await;
    assert!(
        matches!(
            operation(&survey),
            ScriptOperation::Settlement {
                operation: ScriptSettlementOperation::Survey { .. }
            }
        ),
        "expected one survey_site call, saw {:?}",
        survey.request()
    );
    boundary
        .try_enqueue_event(
            survey
                .operation_result(settlement_outcome(
                    1,
                    ScriptSettlementResult::Survey {
                        survey: Box::new(survey_fixture()),
                    },
                ))
                .expect("survey result"),
        )
        .expect("deliver survey result");
    let surveyed = next_command(boundary, "survey write bundle").await;
    deliver_storage_batch(boundary, surveyed, 4).await;
    expect_chat(
        boundary,
        player_id,
        "Survey settlement: plots=4000 water=96 claimed=false chunks=loaded tags=[grassland].",
    )
    .await;

    run_player_command(
        boundary,
        player_id,
        context,
        "settlement project hamlet solaris:house_small here",
    );
    let intent = next_command(boundary, "prepare intent bundle").await;
    let projected = mutation_value(&intent, "building:hamlet:house_sm_1")
        .expect("the project intent must persist the projected building record");
    assert!(
        projected.contains("projected"),
        "the intent bundle must carry the projected state, saw {projected:?}"
    );
    deliver_storage_batch(boundary, intent, 5).await;
}

/// Drive `create`/`adopt`/`populate` until the package has one spawn call
/// admitted against a durably pending resident-site intent.
async fn drive_to_pending_spawn(
    boundary: &ScriptBoundary,
    player_id: ScriptPlayerId,
    context: &ScriptPlayerContext,
) -> PendingSpawn {
    run_player_command(
        boundary,
        player_id,
        context,
        "settlement create hamlet small",
    );
    let create = next_command(boundary, "create bundle").await;
    deliver_storage_batch(boundary, create, 1).await;
    expect_chat(boundary, player_id, "Founded hamlet (small hamlet).").await;

    run_player_command(
        boundary,
        player_id,
        context,
        &format!("settlement adopt {SETTLEMENT} {SITE_ID}"),
    );
    let query = next_command(boundary, "site query").await;
    match operation(&query) {
        ScriptOperation::Settlement {
            operation: ScriptSettlementOperation::QuerySite { site_id, .. },
        } => assert_eq!(site_id, SITE_ID),
        other => panic!("expected one query_settlement_site call, saw {other:?}"),
    }
    boundary
        .try_enqueue_event(
            query
                .operation_result(settlement_outcome(
                    1,
                    ScriptSettlementResult::Site {
                        site: Box::new(site_fixture()),
                    },
                ))
                .expect("site snapshot result"),
        )
        .expect("deliver site snapshot result");
    let adopt = next_command(boundary, "site write bundle").await;
    let site = mutation_value(&adopt, SITE_KEY).expect("the site snapshot must be persisted");
    assert!(!site.is_empty(), "the adopted site must be stored");
    deliver_storage_batch(boundary, adopt, 2).await;
    expect_chat(
        boundary,
        player_id,
        "Adopted site-0001 (hamlet): 0 buildings, 1 points of interest, revision 1.",
    )
    .await;

    run_player_command(boundary, player_id, context, "settlement populate hamlet");
    let reserve_intent = next_command(boundary, "reserve intent bundle").await;
    let ops = mutation_value(&reserve_intent, OPS_KEY)
        .expect("the reserve intent must persist the index");
    assert!(
        ops.contains("reserve_poi") && ops.contains(POI_ID),
        "the reserve intent must name its operation and point of interest, saw {ops:?}"
    );
    deliver_storage_batch(boundary, reserve_intent, 3).await;

    let reserve = next_command(boundary, "reserve call").await;
    match operation(&reserve) {
        ScriptOperation::Settlement {
            operation:
                ScriptSettlementOperation::ReserveResidentSite {
                    site_id,
                    poi_id,
                    expected_site_revision,
                    ..
                },
        } => {
            assert_eq!(site_id, SITE_ID);
            assert_eq!(poi_id, POI_ID);
            assert_eq!(*expected_site_revision, 1);
        }
        other => panic!("expected one reserve_resident_site call, saw {other:?}"),
    }
    boundary
        .try_enqueue_event(
            reserve
                .operation_result(resident_site_outcome(1, TOKEN))
                .expect("reservation result"),
        )
        .expect("deliver reservation result");

    let spawn_intent = next_command(boundary, "spawn intent bundle").await;
    deliver_storage_batch(boundary, spawn_intent, 4).await;
    let spawn = next_command(boundary, "spawn call").await;
    let (operation_id, spawn_site_token) = match operation(&spawn) {
        ScriptOperation::Resident {
            operation:
                ScriptResidentOperation::Spawn {
                    operation_id,
                    spawn_site_token,
                    ..
                },
        } => (operation_id.clone(), spawn_site_token.clone()),
        other => panic!("expected one spawn_resident call, saw {other:?}"),
    };
    assert_eq!(spawn_site_token, TOKEN);
    PendingSpawn {
        admitted: spawn,
        operation_id,
    }
}

/// Drive `create`/`adopt`/`survey` until the package holds one adopted hamlet
/// with a surveyed plot.
async fn drive_to_surveyed_hamlet(
    boundary: &ScriptBoundary,
    player_id: ScriptPlayerId,
    context: &ScriptPlayerContext,
) {
    run_player_command(
        boundary,
        player_id,
        context,
        "settlement create hamlet small",
    );
    let create = next_command(boundary, "create bundle").await;
    deliver_storage_batch(boundary, create, 1).await;
    expect_chat(boundary, player_id, "Founded hamlet (small hamlet).").await;

    run_player_command(
        boundary,
        player_id,
        context,
        &format!("settlement adopt {SETTLEMENT} {SITE_ID}"),
    );
    let query = next_command(boundary, "site query").await;
    match operation(&query) {
        ScriptOperation::Settlement {
            operation: ScriptSettlementOperation::QuerySite { site_id, .. },
        } => assert_eq!(site_id, SITE_ID),
        other => panic!("expected one query_settlement_site call, saw {other:?}"),
    }
    boundary
        .try_enqueue_event(
            query
                .operation_result(settlement_outcome(
                    1,
                    ScriptSettlementResult::Site {
                        site: Box::new(site_fixture()),
                    },
                ))
                .expect("site snapshot result"),
        )
        .expect("deliver site snapshot result");
    let adopt = next_command(boundary, "site write bundle").await;
    deliver_storage_batch(boundary, adopt, 2).await;
    expect_chat(
        boundary,
        player_id,
        "Adopted site-0001 (hamlet): 0 buildings, 1 points of interest, revision 1.",
    )
    .await;

    run_player_command(
        boundary,
        player_id,
        context,
        "settlement survey hamlet plot",
    );
    let survey_intent = next_command(boundary, "survey intent bundle").await;
    deliver_storage_batch(boundary, survey_intent, 3).await;
    let survey = next_command(boundary, "survey call").await;
    assert!(
        matches!(
            operation(&survey),
            ScriptOperation::Settlement {
                operation: ScriptSettlementOperation::Survey { .. }
            }
        ),
        "expected one survey_site call, saw {:?}",
        survey.request()
    );
    boundary
        .try_enqueue_event(
            survey
                .operation_result(settlement_outcome(
                    1,
                    ScriptSettlementResult::Survey {
                        survey: Box::new(survey_fixture()),
                    },
                ))
                .expect("survey result"),
        )
        .expect("deliver survey result");
    let surveyed = next_command(boundary, "survey write bundle").await;
    deliver_storage_batch(boundary, surveyed, 4).await;
    expect_chat(
        boundary,
        player_id,
        "Survey settlement: plots=4000 water=96 claimed=false chunks=loaded tags=[grassland].",
    )
    .await;
}

/// Drive `create`/`adopt`/`survey`/`project`/`fund`/`build` until the package
/// holds one committed `solaris:warehouse`, returning the building name it
/// minted for it and the core structure id it was committed under.
async fn drive_to_committed_warehouse(
    boundary: &ScriptBoundary,
    player_id: ScriptPlayerId,
    context: &ScriptPlayerContext,
) -> (String, String) {
    drive_to_surveyed_hamlet(boundary, player_id, context).await;

    run_player_command(
        boundary,
        player_id,
        context,
        &format!("settlement project {SETTLEMENT} {WAREHOUSE_BLUEPRINT} here"),
    );
    let intent = next_command(boundary, "prepare intent bundle").await;
    deliver_storage_batch(boundary, intent, 5).await;

    let prepare = next_command(boundary, "prepare call").await;
    match operation(&prepare) {
        ScriptOperation::Settlement {
            operation: ScriptSettlementOperation::PrepareStructure { blueprint_id, .. },
        } => assert_eq!(blueprint_id, WAREHOUSE_BLUEPRINT),
        other => panic!("expected one prepare_structure call, saw {other:?}"),
    }
    boundary
        .try_enqueue_event(
            prepare
                .operation_result(settlement_outcome(
                    2,
                    ScriptSettlementResult::Structure {
                        structure: Box::new(structure_fixture(ScriptStructureState::Prepared, 1)),
                    },
                ))
                .expect("prepared structure result"),
        )
        .expect("deliver prepared structure result");
    // The package persists the plan core handed it; funding reads that value
    // back rather than re-deriving a plan of its own.
    let projected = next_command(boundary, "projected write bundle").await;
    let plan_key = format!("plan:{SETTLEMENT}:{WAREHOUSE_BUILDING}");
    let plan = mutation_value(&projected, &plan_key)
        .expect("the projected write must persist the plan core returned");
    deliver_storage_batch(boundary, projected, 6).await;
    expect_chat(
        boundary,
        player_id,
        &format!(
            "{WAREHOUSE_BUILDING} projected ({WAREHOUSE_BLUEPRINT}, {} stages). Fund it: /settlement fund {SETTLEMENT} {WAREHOUSE_BUILDING}",
            WAREHOUSE_STAGES.len()
        ),
    )
    .await;

    run_player_command(
        boundary,
        player_id,
        context,
        &format!("settlement fund {SETTLEMENT} {WAREHOUSE_BUILDING}"),
    );
    let plan_read = next_command(boundary, "plan read").await;
    assert!(
        matches!(
            plan_read.request(),
            ScriptCommand::PluginStorageGet { request } if request.key() == plan_key
        ),
        "the fund command must reload the stored plan, saw {:?}",
        plan_read.request()
    );
    boundary
        .try_enqueue_event(
            plan_read
                .plugin_storage_get_result(Some(&plan), Some(1))
                .expect("stored plan result"),
        )
        .expect("deliver stored plan");
    // Funding reserves the plan's materials out of the payer's own canonical
    // inventory, so that inventory is read before anything is reserved.
    let fund_read = next_command(boundary, "fund inventory read").await;
    match operation(&fund_read) {
        ScriptOperation::Inventory {
            operation:
                ScriptOwnedInventoryOperation::Query {
                    endpoint: ScriptInventoryEndpoint::PlayerInventory { player_id: payer },
                    ..
                },
        } => assert_eq!(
            *payer, PLAYER,
            "the fund must read the canonical inventory of the player who pays"
        ),
        other => panic!("expected one player_inventory read, saw {other:?}"),
    }
    boundary
        .try_enqueue_event(
            fund_read
                .operation_result(owned_inventory_outcome(
                    3,
                    ScriptOwnedInventoryResult::Snapshot {
                        inventory: player_inventory_fixture(),
                    },
                ))
                .expect("player inventory snapshot"),
        )
        .expect("deliver player inventory snapshot");

    let fund_intent = next_command(boundary, "fund intent bundle").await;
    deliver_storage_batch(boundary, fund_intent, 7).await;
    let reserve = next_command(boundary, "reserve call").await;
    match operation(&reserve) {
        ScriptOperation::Inventory {
            operation:
                ScriptOwnedInventoryOperation::Reserve {
                    endpoint: ScriptInventoryEndpoint::PlayerInventory { player_id: payer },
                    ..
                },
        } => assert_eq!(*payer, PLAYER),
        other => panic!("expected one reserve_inventory_items call, saw {other:?}"),
    }
    boundary
        .try_enqueue_event(
            reserve
                .operation_result(owned_inventory_outcome(
                    4,
                    ScriptOwnedInventoryResult::Reservation {
                        reservation: reservation_fixture(),
                    },
                ))
                .expect("reservation result"),
        )
        .expect("deliver reservation result");
    let funded = next_command(boundary, "funded write bundle").await;
    let building_key = format!("building:{SETTLEMENT}:{WAREHOUSE_BUILDING}");
    let building =
        mutation_value(&funded, &building_key).expect("the funded write must persist the building");
    deliver_storage_batch(boundary, funded, 8).await;
    expect_chat(
        boundary,
        player_id,
        &format!(
            "Reserved real materials for {WAREHOUSE_BUILDING} ({RESERVATION}). Build: /settlement build {SETTLEMENT} {WAREHOUSE_BUILDING}"
        ),
    )
    .await;

    run_player_command(
        boundary,
        player_id,
        context,
        &format!("settlement build {SETTLEMENT} {WAREHOUSE_BUILDING}"),
    );
    let building_read = next_command(boundary, "building read").await;
    assert!(
        matches!(
            building_read.request(),
            ScriptCommand::PluginStorageGet { request } if request.key() == building_key
        ),
        "the build command must reload the stored building, saw {:?}",
        building_read.request()
    );
    boundary
        .try_enqueue_event(
            building_read
                .plugin_storage_get_result(Some(&building), Some(1))
                .expect("stored building result"),
        )
        .expect("deliver stored building");

    // One advance per stage: every stage is one work unit, so a single receipt
    // closes it and the package walks the plan until it is exhausted.
    let mut structure_revision = 1;
    for (index, stage) in WAREHOUSE_STAGES.iter().enumerate() {
        let advance_intent = next_command(boundary, "advance intent bundle").await;
        deliver_storage_batch(boundary, advance_intent, 9 + index as u64).await;
        let advance = next_command(boundary, "advance call").await;
        match operation(&advance) {
            ScriptOperation::Settlement {
                operation:
                    ScriptSettlementOperation::AdvanceStructure {
                        structure_id,
                        stage: requested,
                        expected_revision,
                        work_units,
                        ..
                    },
            } => {
                assert_eq!(structure_id, STRUCTURE_ID);
                assert_eq!(
                    requested, stage,
                    "the package must advance the plan in its authored order"
                );
                assert_eq!(
                    *work_units, 1,
                    "one call must close exactly one one-unit stage"
                );
                assert_eq!(
                    *expected_revision, structure_revision,
                    "every advance must be fenced on the revision the previous receipt reported"
                );
            }
            other => panic!("expected one advance_structure call, saw {other:?}"),
        }
        structure_revision += 1;
        boundary
            .try_enqueue_event(
                advance
                    .operation_result(settlement_outcome(
                        structure_revision,
                        ScriptSettlementResult::Receipt {
                            receipt: Box::new(receipt_fixture(
                                stage,
                                index as u64 + 1,
                                structure_revision,
                            )),
                        },
                    ))
                    .expect("advance receipt"),
            )
            .expect("deliver advance receipt");
    }

    // The plan is exhausted, so the package asks core for the authoritative
    // commit state instead of trusting its own counter.
    let verify = next_command(boundary, "verify intent bundle").await;
    deliver_storage_batch(boundary, verify, 13).await;
    let status = next_command(boundary, "structure status read").await;
    match operation(&status) {
        ScriptOperation::Settlement {
            operation: ScriptSettlementOperation::Status { structure_id },
        } => assert_eq!(structure_id, STRUCTURE_ID),
        other => panic!("expected one structure_status read, saw {other:?}"),
    }
    boundary
        .try_enqueue_event(
            status
                .operation_result(settlement_outcome(
                    structure_revision,
                    ScriptSettlementResult::Structure {
                        structure: Box::new(structure_fixture(
                            ScriptStructureState::Committed,
                            structure_revision,
                        )),
                    },
                ))
                .expect("committed structure result"),
        )
        .expect("deliver committed structure result");
    let committed = next_command(boundary, "committed write bundle").await;
    deliver_storage_batch(boundary, committed, 14).await;
    expect_chat(
        boundary,
        player_id,
        &format!(
            "{WAREHOUSE_BUILDING} committed ({WAREHOUSE_BLUEPRINT}) at revision {structure_revision}."
        ),
    )
    .await;

    (WAREHOUSE_BUILDING.to_owned(), STRUCTURE_ID.to_owned())
}

/// Drive `populate` until the package has one spawned resident, returning the
/// durable name it minted and the value it persisted for that resident.
async fn drive_to_resident(
    boundary: &ScriptBoundary,
    player_id: ScriptPlayerId,
    context: &ScriptPlayerContext,
) -> SpawnedResident {
    run_player_command(
        boundary,
        player_id,
        context,
        &format!("settlement populate {SETTLEMENT}"),
    );
    let reserve_intent = next_command(boundary, "reserve intent bundle").await;
    let ops = mutation_value(&reserve_intent, OPS_KEY)
        .expect("the reserve intent must persist the index");
    assert!(
        ops.contains("reserve_poi") && ops.contains(POI_ID),
        "the reserve intent must name its operation and point of interest, saw {ops:?}"
    );
    deliver_storage_batch(boundary, reserve_intent, 15).await;

    let reserve = next_command(boundary, "reserve call").await;
    match operation(&reserve) {
        ScriptOperation::Settlement {
            operation:
                ScriptSettlementOperation::ReserveResidentSite {
                    site_id, poi_id, ..
                },
        } => {
            assert_eq!(site_id, SITE_ID);
            assert_eq!(poi_id, POI_ID);
        }
        other => panic!("expected one reserve_resident_site call, saw {other:?}"),
    }
    boundary
        .try_enqueue_event(
            reserve
                .operation_result(resident_site_outcome(2, TOKEN))
                .expect("reservation result"),
        )
        .expect("deliver reservation result");

    let spawn_intent = next_command(boundary, "spawn intent bundle").await;
    deliver_storage_batch(boundary, spawn_intent, 16).await;
    let spawn = next_command(boundary, "spawn call").await;
    match operation(&spawn) {
        ScriptOperation::Resident {
            operation:
                ScriptResidentOperation::Spawn {
                    spawn_site_token, ..
                },
        } => assert_eq!(spawn_site_token, TOKEN),
        other => panic!("expected one spawn_resident call, saw {other:?}"),
    }
    boundary
        .try_enqueue_event(
            spawn
                .operation_result(resident_outcome(
                    2,
                    ScriptResidentResult::Snapshot {
                        resident: resident_fixture(),
                    },
                ))
                .expect("spawned resident result"),
        )
        .expect("deliver spawned resident result");

    let spawned = next_command(boundary, "resident write bundle").await;
    let name = mutation_keys(&spawned)
        .into_iter()
        .find_map(|key| {
            key.strip_prefix(&format!("resident:{SETTLEMENT}:"))
                .map(str::to_owned)
        })
        .expect("the spawn write must persist the resident under its durable name");
    let stored = mutation_value(&spawned, &format!("resident:{SETTLEMENT}:{name}"))
        .expect("the spawn write must persist the resident value");
    deliver_storage_batch(boundary, spawned, 17).await;
    expect_chat(
        boundary,
        player_id,
        &format!(
            "{name} settled in {SETTLEMENT} (alive_unloaded), home {POI_ID}. House capacity is tracked by that home POI."
        ),
    )
    .await;

    SpawnedResident { name, stored }
}

/// Assign `hauling` to the spawned resident and stop at the admitted bind: the
/// assignment half of a haul is a container of the settlement's own warehouse,
/// so the package binds that container before it builds any work order. Both
/// hauling cases answer this same bind, and only their answer differs.
async fn drive_to_haul_bind(
    boundary: &ScriptBoundary,
    player_id: ScriptPlayerId,
    context: &ScriptPlayerContext,
    structure_id: &str,
    resident: &SpawnedResident,
) -> AdmittedScriptCommand {
    run_player_command(
        boundary,
        player_id,
        context,
        &format!("settlement job {SETTLEMENT} {} hauling", resident.name),
    );

    let read = next_command(boundary, "resident read").await;
    assert!(
        matches!(
            read.request(),
            ScriptCommand::PluginStorageGet { request }
                if request.key() == format!("resident:{SETTLEMENT}:{}", resident.name)
        ),
        "the job command must reload the resident it assigns, saw {:?}",
        read.request()
    );
    boundary
        .try_enqueue_event(
            read.plugin_storage_get_result(Some(&resident.stored), Some(1))
                .expect("stored resident result"),
        )
        .expect("deliver stored resident");

    let bind = next_command(boundary, "warehouse bind call").await;
    match operation(&bind) {
        ScriptOperation::Settlement {
            operation:
                ScriptSettlementOperation::BindWarehouse {
                    structure_id: bound,
                    container_id,
                    ..
                },
        } => {
            assert_eq!(
                bound, structure_id,
                "the bind must name the structure the settlement committed"
            );
            assert_eq!(
                *container_id, 0,
                "the authored container ordinal is the package's only choice"
            );
        }
        other => panic!("expected one bind_warehouse call before any work order, saw {other:?}"),
    }
    bind
}

async fn start_host(plugins_root: &Path) -> (ScriptBoundary, mc_script::LuaHost) {
    copy_plugin(plugins_root);
    let (boundary, host) = mc_script::start_lua_host(
        mc_script::LuaHostConfig::new(plugins_root).strict_discovery(true),
    )
    .expect("start the shipped settlements plugin");
    assert_eq!(host.loaded_plugins(), 1);

    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .expect("enqueue server start");
    let boot = next_command(&boundary, "settlements boot read").await;
    assert_eq!(boot.plugin_id(), PLUGIN);
    assert!(
        matches!(
            boot.request(),
            ScriptCommand::PluginStorageGet { request } if request.key() == INDEX_KEY
        ),
        "the package must read its settlement index on start, saw {:?}",
        boot.request()
    );
    boundary
        .try_enqueue_event(
            boot.plugin_storage_get_result(None, None)
                .expect("empty settlements index"),
        )
        .expect("deliver empty settlements index");
    (boundary, host)
}

async fn stop_host(boundary: ScriptBoundary, host: mc_script::LuaHost) {
    drop(boundary);
    tokio::task::spawn_blocking(move || host.join())
        .await
        .expect("Lua host join task")
        .expect("Lua host thread");
}

fn copy_plugin(destination_root: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../solaris-default-plugins")
        .join(PLUGIN);
    assert!(
        source.is_dir(),
        "sibling plugin checkout missing at {}: clone solaris-default-plugins next to solaris",
        source.display()
    );
    let destination = destination_root.join(PLUGIN);
    copy_directory(&source, &destination);
}

/// The whole package, so a shipped client artifact is deployed with it and a
/// new package file cannot silently fall out of this gate.
fn copy_directory(source: &Path, destination: &Path) {
    std::fs::create_dir(destination).expect("create copied plugin directory");
    for entry in std::fs::read_dir(source).expect("read shipped plugin directory") {
        let entry = entry.expect("read shipped plugin entry");
        let target = destination.join(entry.file_name());
        if entry
            .file_type()
            .expect("shipped plugin entry type")
            .is_dir()
        {
            copy_directory(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap_or_else(|error| {
                panic!(
                    "copy shipped {PLUGIN}/{}: {error}",
                    entry.file_name().display()
                )
            });
        }
    }
}

fn player_context() -> ScriptPlayerContext {
    ScriptPlayerContext::new(PLAYER_UUID, "Founder", false, 0.0, 64.0, 0.0)
}

fn site_fixture() -> ScriptSettlementSite {
    ScriptSettlementSite::new(
        SITE_ID.to_owned(),
        ScriptSiteProvenance::Authored,
        ScriptSiteVariant::Hamlet,
        1,
        true,
        [0, 64, 0],
        [16, 8, 16],
        Vec::new(),
        vec![ScriptSettlementPoi::new(
            POI_ID.to_owned(),
            ScriptSitePoiKind::Home,
            [0, 64, 0],
            1,
            ScriptSitePoiState::Free,
        )],
        Vec::new(),
    )
}

/// The plot survey the package requires before it will project anything.
/// Bounds match `/settlement survey hamlet plot` at the test player's origin.
fn survey_fixture() -> ScriptSurveySnapshot {
    ScriptSurveySnapshot::new(
        "minecraft:overworld".to_owned(),
        ScriptSurveyBounds::new([-32, 48, -32], [31, 79, 31]).expect("survey bounds"),
        1,
        ScriptChunkAvailability::Loaded,
        "survey-token-1".to_owned(),
        4000,
        96,
        false,
        0,
        vec!["minecraft:plains".to_owned()],
        vec!["grassland".to_owned()],
    )
}

fn resident_site_outcome(revision: u64, token: &str) -> ScriptOperationOutcome {
    settlement_outcome(
        revision,
        ScriptSettlementResult::ResidentSite {
            reservation: Box::new(ScriptResidentSiteReservation::new(
                SITE_ID.to_owned(),
                POI_ID.to_owned(),
                token.to_owned(),
                1,
            )),
        },
    )
}

/// The warehouse as core reports it: four one-unit stages, so one receipt
/// carries a whole stage and the plan's consumed/remaining split follows the
/// state core reports.
fn structure_fixture(state: ScriptStructureState, revision: u64) -> ScriptStructureSnapshot {
    let plan_materials = vec![ScriptStructureMaterial::new(
        WAREHOUSE_MATERIAL.to_owned(),
        WAREHOUSE_STAGES.len() as u64,
    )];
    let stages = WAREHOUSE_STAGES
        .iter()
        .map(|stage| {
            ScriptStructureStagePlan::new(
                (*stage).to_owned(),
                1,
                1,
                vec![ScriptStructureMaterial::new(
                    WAREHOUSE_MATERIAL.to_owned(),
                    1,
                )],
            )
        })
        .collect();
    let (consumed, remaining) = match state {
        ScriptStructureState::Committed => (plan_materials, Vec::new()),
        _ => (Vec::new(), plan_materials),
    };
    ScriptStructureSnapshot::new(
        STRUCTURE_ID.to_owned(),
        WAREHOUSE_BLUEPRINT.to_owned(),
        SITE_ID.to_owned(),
        state,
        revision,
        [0, 64, 0],
        0,
        [9, 3, 9],
        stages,
        PLAN_HASH.to_owned(),
        Some(RESERVATION.to_owned()),
        0,
        consumed,
        remaining,
        None,
    )
}

/// One committed portion of the warehouse: the stage the package asked for,
/// one block and one work unit of it.
fn receipt_fixture(stage: &str, sequence: u64, revision: u64) -> ScriptStructureReceipt {
    ScriptStructureReceipt::new(
        STRUCTURE_ID.to_owned(),
        stage.to_owned(),
        sequence,
        1,
        1,
        vec![ScriptStructureMaterial::new(
            WAREHOUSE_MATERIAL.to_owned(),
            1,
        )],
        revision,
    )
}

/// The payer's own canonical inventory: every unit the plan reserves is really
/// held, so funding reserves something that exists.
fn player_inventory_fixture() -> ScriptOwnedInventorySnapshot {
    ScriptOwnedInventorySnapshot::new(
        ScriptInventoryEndpoint::PlayerInventory { player_id: PLAYER },
        ScriptInventoryFence::try_new(1, SNAPSHOT_HASH.to_owned()).expect("player inventory fence"),
        vec![ScriptInventorySlot::new(
            9,
            Some(ScriptInventoryItem::new(
                WAREHOUSE_MATERIAL.to_owned(),
                WAREHOUSE_STAGES.len() as u32,
                None,
                Vec::new(),
                None,
                None,
            )),
        )],
    )
}

/// The reservation core commits for the funding: the plan's units taken from
/// the payer and bound to the structure they are for.
fn reservation_fixture() -> ScriptInventoryReservationSnapshot {
    ScriptInventoryReservationSnapshot::new(
        RESERVATION.to_owned(),
        ScriptInventoryEndpoint::PlayerInventory { player_id: PLAYER },
        PLAN_HASH.to_owned(),
        vec![ScriptInventoryReservationQuantity::new(
            WAREHOUSE_MATERIAL.to_owned(),
            WAREHOUSE_STAGES.len() as u64,
            0,
            0,
            WAREHOUSE_STAGES.len() as u64,
        )],
        Some(STRUCTURE_ID.to_owned()),
        false,
        1,
    )
}

/// The resident core materialised: its handle and entity UUID both derive from
/// the generation the spawn names, and it has not been loaded yet.
fn resident_fixture() -> ScriptResidentSnapshot {
    ScriptResidentSnapshot::new(
        resident_handle_for_generation(PLUGIN, RESIDENT_GENERATION).expect("core resident handle"),
        resident_uuid(),
        ScriptResidentLifecycle::AliveUnloaded,
        1,
        Some(RESIDENT_GENERATION.to_owned()),
        ScriptResidentPois::new(Some(POI_ID.to_owned()), None, None),
        None,
    )
}

/// The entity identity core mints for that generation, as a canonical
/// lowercase hyphenated UUID.
fn resident_uuid() -> String {
    let bytes = resident_entity_uuid(RESIDENT_GENERATION).expect("core resident entity uuid");
    let mut text = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// The worker's own equipment read before work is assigned: a haul's cargo is
/// the worker's carry, so the equipment is the revision source and nothing
/// else.
fn resident_equipment_fixture(handle: &str, revision: u64) -> ScriptOwnedInventorySnapshot {
    ScriptOwnedInventorySnapshot::new(
        ScriptInventoryEndpoint::ResidentEquipment {
            handle: handle.to_owned(),
        },
        ScriptInventoryFence::try_new(revision, SNAPSHOT_HASH.to_owned())
            .expect("resident equipment fence"),
        Vec::new(),
    )
}

fn owned_inventory_outcome(
    revision: u64,
    result: ScriptOwnedInventoryResult,
) -> ScriptOperationOutcome {
    ScriptOperationOutcome::committed(
        revision,
        ScriptOperationPayload::OwnedInventory {
            result: Box::new(result),
        },
    )
    .expect("committed owned-inventory outcome is canonical")
}

fn resident_outcome(revision: u64, result: ScriptResidentResult) -> ScriptOperationOutcome {
    ScriptOperationOutcome::committed(
        revision,
        ScriptOperationPayload::Resident {
            result: Box::new(result),
        },
    )
    .expect("committed resident outcome is canonical")
}

fn settlement_outcome(revision: u64, result: ScriptSettlementResult) -> ScriptOperationOutcome {
    ScriptOperationOutcome::committed(
        revision,
        ScriptOperationPayload::Settlement {
            result: Box::new(result),
        },
    )
    .expect("committed settlement outcome is canonical")
}

async fn deliver_storage_batch(
    boundary: &ScriptBoundary,
    admitted: AdmittedScriptCommand,
    revision: u64,
) {
    assert!(
        matches!(operation(&admitted), ScriptOperation::StorageBatch { .. }),
        "expected one atomic storage batch, saw {:?}",
        admitted.request()
    );
    let outcome = ScriptOperationOutcome::committed(
        revision,
        ScriptOperationPayload::StorageBatch {
            changes: vec![ScriptStorageChange::new(INDEX_KEY.to_owned(), false)],
        },
    )
    .expect("committed storage batch outcome");
    boundary
        .try_enqueue_event(
            admitted
                .operation_result(outcome)
                .expect("storage batch result"),
        )
        .expect("deliver storage batch result");
}

/// Read one persisted value out of an admitted atomic storage batch.
fn mutation_value(admitted: &AdmittedScriptCommand, key: &str) -> Option<String> {
    let ScriptOperation::StorageBatch { mutations, .. } = operation(admitted) else {
        return None;
    };
    mutations.iter().find_map(|mutation| match mutation {
        ScriptStorageMutation::CompareAndSwap {
            key: candidate,
            value,
            ..
        } if candidate == key => Some(value.clone()),
        _ => None,
    })
}

/// The durable operation id one admitted atomic storage batch carries.
fn storage_batch_id(admitted: &AdmittedScriptCommand) -> String {
    let ScriptOperation::StorageBatch { operation_id, .. } = operation(admitted) else {
        panic!(
            "expected an atomic storage batch, saw {:?}",
            admitted.request()
        );
    };
    operation_id.clone()
}

/// Every persisted key named by one admitted atomic storage batch.
fn mutation_keys(admitted: &AdmittedScriptCommand) -> Vec<String> {
    let ScriptOperation::StorageBatch { mutations, .. } = operation(admitted) else {
        panic!(
            "expected an atomic storage batch, saw {:?}",
            admitted.request()
        );
    };
    mutations
        .iter()
        .map(|mutation| match mutation {
            ScriptStorageMutation::CompareAndSwap { key, .. }
            | ScriptStorageMutation::Delete { key, .. } => key.clone(),
            _ => panic!("unexpected storage mutation {mutation:?}"),
        })
        .collect()
}

/// Whether one admitted atomic storage batch deletes `key`.
fn mutation_deleted(admitted: &AdmittedScriptCommand, key: &str) -> bool {
    let ScriptOperation::StorageBatch { mutations, .. } = operation(admitted) else {
        panic!(
            "expected an atomic storage batch, saw {:?}",
            admitted.request()
        );
    };
    mutations.iter().any(|mutation| {
        matches!(mutation, ScriptStorageMutation::Delete { key: candidate, .. } if candidate == key)
    })
}

fn operation(admitted: &AdmittedScriptCommand) -> &ScriptOperation {
    let ScriptCommand::Operation { request } = admitted.request() else {
        panic!(
            "expected an operation command, saw {:?}",
            admitted.request()
        );
    };
    request.operation()
}

fn run_player_command(
    boundary: &ScriptBoundary,
    player_id: ScriptPlayerId,
    context: &ScriptPlayerContext,
    raw: &str,
) {
    assert_eq!(
        boundary
            .try_enqueue_player_command_with_context(player_id, context.clone(), raw)
            .unwrap_or_else(|error| panic!("enqueue {raw:?}: {error:?}")),
        PlayerCommandAdmission::Enqueued,
        "{raw:?} must be owned by the settlements package"
    );
}

async fn next_command(boundary: &ScriptBoundary, what: &str) -> AdmittedScriptCommand {
    let command = tokio::time::timeout(COMMAND_WAIT, boundary.recv_command())
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
        .unwrap_or_else(|| panic!("script host closed while waiting for {what}"));
    boundary
        .accept_host_command(command)
        .unwrap_or_else(|error| panic!("admit {what}: {error:?}"))
}

async fn expect_chat(boundary: &ScriptBoundary, player_id: ScriptPlayerId, expected: &str) {
    let admitted = next_command(boundary, "settlements chat reply").await;
    assert_eq!(admitted.plugin_id(), PLUGIN);
    assert!(
        matches!(
            admitted.request(),
            ScriptCommand::SendChatMessage { player_id: target, message }
                if *target == player_id && message == expected
        ),
        "the package must reply {expected:?}, saw {:?}",
        admitted.request()
    );
}
