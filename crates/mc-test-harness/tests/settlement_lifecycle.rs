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
    ScriptChunkAvailability, ScriptCommand, ScriptEvent, ScriptOperation, ScriptOperationFailure,
    ScriptOperationOutcome, ScriptOperationPayload, ScriptPlayerContext, ScriptPlayerId,
    ScriptResidentOperation, ScriptResidentSiteReservation, ScriptSettlementOperation,
    ScriptSettlementPoi, ScriptSettlementResult, ScriptSettlementSite, ScriptSitePoiKind,
    ScriptSitePoiState, ScriptSiteVariant, ScriptStorageChange, ScriptStorageMutation,
    ScriptSurveyBounds, ScriptSurveySnapshot,
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

/// One resident spawn that is durably pending in the package.
struct PendingSpawn {
    admitted: AdmittedScriptCommand,
    operation_id: String,
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
    std::fs::create_dir(&destination).expect("create copied plugin directory");
    for file in ["plugin.toml", "main.lua", "config.toml"] {
        std::fs::copy(source.join(file), destination.join(file))
            .unwrap_or_else(|error| panic!("copy shipped {PLUGIN}/{file}: {error}"));
    }
}

fn player_context() -> ScriptPlayerContext {
    ScriptPlayerContext::new(PLAYER_UUID, "Founder", false, 0.0, 64.0, 0.0)
}

fn site_fixture() -> ScriptSettlementSite {
    ScriptSettlementSite::new(
        SITE_ID.to_owned(),
        ScriptSiteVariant::Hamlet,
        1,
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
