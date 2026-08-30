//! Integration coverage for the first-party Solaris standard plugin pack.
//!
//! `examples/plugins/standard-pack/README.md` ships five independent API 0.6
//! server-only packages. Every test below copies the whole pack into one temp
//! root, strict-loads it, satisfies each plugin's `server.started` contract,
//! and then drives representative player commands through the script boundary
//! exactly like `plugin_examples.rs` does: enqueue with a server-authored
//! `ScriptPlayerContext`, receive the emitted host command, deliver its result,
//! and observe the chat reply the player actually sees.

use std::path::Path;
use std::time::Duration;

use mc_script::{
    AdmittedScriptCommand, PlayerCommandAdmission, ScriptBoundary, ScriptCommand, ScriptEvent,
    ScriptGameMode, ScriptPlayerContext, ScriptPlayerId,
};

/// Startup order recommended by `examples/plugins/standard-pack/README.md`.
const STANDARD_PACK: [&str; 5] = [
    "solaris-permissions",
    "solaris-essentials",
    "solaris-economy",
    "solaris-towns",
    "solaris-audit",
];

const COMMAND_WAIT: Duration = Duration::from_secs(5);

const PLAYER_UUID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const OPERATOR_UUID: &str = "11111111-2222-3333-4444-555555555555";
const PEER_UUID: &str = "66666666-7777-8888-9999-000000000000";

/// The storage key form each plugin derives from a hyphenated player UUID.
fn normalized(uuid: &str) -> String {
    uuid.replace('-', "").to_ascii_lowercase()
}

#[tokio::test]
async fn standard_pack_strict_loads_all_five_plugins_and_satisfies_startup_contracts() {
    let plugins = tempfile::tempdir().expect("standard pack tempdir");
    copy_standard_pack(plugins.path());
    let (boundary, host) = start_standard_pack(plugins.path(), None).await;

    let mut roots = boundary.player_command_roots();
    roots.sort_unstable();
    assert_eq!(
        roots,
        vec![
            "audit",
            "back",
            "delhome",
            "delwarp",
            "econadmin",
            "home",
            "money",
            "msg",
            "pay",
            "perm",
            "reply",
            "sethome",
            "setspawn",
            "setwarp",
            "spawn",
            "town",
            "tpa",
            "tpaccept",
            "warp",
        ]
    );
    // The pack README is explicit that API 0.6 has no cross-plugin permission
    // query, so every pack root stays a plain player command and each plugin
    // keeps its own in-handler operator check.
    assert!(
        boundary.operator_command_roots().is_empty(),
        "standard pack roots must stay player commands until a cross-plugin permission query exists"
    );

    // Each storage-backed pack plugin must have finished loading: all four
    // answer from their loaded state, and a stray startup command from
    // solaris-essentials (which subscribes to no startup event) would surface
    // here as the wrong command owner.
    let player_id = ScriptPlayerId::new(7);
    let member = ScriptPlayerContext::new(PLAYER_UUID, "Member", false, 0.0, 64.0, 0.0);
    let operator = ScriptPlayerContext::new(OPERATOR_UUID, "Operator", true, 0.0, 64.0, 0.0);

    run_player_command(&boundary, player_id, &member, "perm groups");
    expect_chat(
        &boundary,
        "solaris-permissions",
        player_id,
        "Groups: admin, moderator, player",
    )
    .await;

    run_player_command(&boundary, player_id, &member, "money");
    expect_chat(
        &boundary,
        "solaris-economy",
        player_id,
        "Balance: 100 coins.",
    )
    .await;

    run_player_command(&boundary, player_id, &member, "town info");
    expect_chat(&boundary, "solaris-towns", player_id, "Town not found.").await;

    run_player_command(&boundary, player_id, &operator, "audit 1");
    expect_chat(
        &boundary,
        "solaris-audit",
        player_id,
        "No matching bounded audit records.",
    )
    .await;

    stop_standard_pack(boundary, host).await;
}

#[tokio::test]
async fn standard_pack_permissions_promotes_a_player_through_a_storage_cas_roundtrip() {
    let plugins = tempfile::tempdir().expect("standard pack tempdir");
    copy_standard_pack(plugins.path());
    let (boundary, host) = start_standard_pack(plugins.path(), None).await;

    let player_id = ScriptPlayerId::new(11);
    let operator_id = ScriptPlayerId::new(12);
    let member = ScriptPlayerContext::new(PLAYER_UUID, "Member", false, 8.0, 70.0, -4.0);
    let operator = ScriptPlayerContext::new(OPERATOR_UUID, "Operator", true, 8.0, 70.0, -4.0);

    // The configured default group grants the pack's own use nodes.
    run_player_command(
        &boundary,
        player_id,
        &member,
        "perm check solaris.essentials.use",
    );
    expect_chat(
        &boundary,
        "solaris-permissions",
        player_id,
        "Permission granted.",
    )
    .await;
    // ... but not the moderator-only audit lookup node.
    run_player_command(
        &boundary,
        player_id,
        &member,
        "perm check solaris.audit.lookup",
    );
    expect_chat(
        &boundary,
        "solaris-permissions",
        player_id,
        "Permission denied.",
    )
    .await;

    // Permission gate: a non-operator can never move assignments.
    run_player_command(&boundary, player_id, &member, "perm user me set moderator");
    expect_chat(
        &boundary,
        "solaris-permissions",
        player_id,
        "Only an operator can change groups.",
    )
    .await;

    // Operator write commits through a first compare-and-swap on the new key.
    run_player_command(
        &boundary,
        operator_id,
        &operator,
        &format!("perm user {PLAYER_UUID} set moderator"),
    );
    let assignment_key = normalized(PLAYER_UUID);
    let admitted = next_command(&boundary, "permission assignment commit").await;
    assert_eq!(admitted.plugin_id(), "solaris-permissions");
    assert!(
        matches!(
            admitted.request(),
            ScriptCommand::PluginStorageCompareAndSwap { request }
                if request.key() == "assignments-v1"
                    && request.expected_version().is_none()
                    && request.value() == format!("v1|{assignment_key},moderator")
        ),
        "assignment commit must create the durable key, saw {:?}",
        admitted.request()
    );
    boundary
        .try_enqueue_event(
            admitted
                .plugin_storage_cas_result(true, Some(1))
                .expect("assignment commit result"),
        )
        .expect("deliver assignment commit result");
    expect_chat(
        &boundary,
        "solaris-permissions",
        operator_id,
        "Group set to moderator.",
    )
    .await;

    // The persisted assignment is now live for the promoted player.
    run_player_command(
        &boundary,
        player_id,
        &member,
        "perm check solaris.audit.lookup",
    );
    expect_chat(
        &boundary,
        "solaris-permissions",
        player_id,
        "Permission granted.",
    )
    .await;

    // Clearing rewrites the same key at the committed version.
    run_player_command(
        &boundary,
        operator_id,
        &operator,
        &format!("perm user {PLAYER_UUID} clear"),
    );
    let admitted = next_command(&boundary, "permission assignment reset").await;
    assert_eq!(admitted.plugin_id(), "solaris-permissions");
    assert!(
        matches!(
            admitted.request(),
            ScriptCommand::PluginStorageCompareAndSwap { request }
                if request.key() == "assignments-v1"
                    && request.expected_version() == Some(1)
                    && request.request_id() == "save-v1"
                    && request.value() == "v1|"
        ),
        "assignment reset must compare-and-swap against the committed version, saw {:?}",
        admitted.request()
    );
    boundary
        .try_enqueue_event(
            admitted
                .plugin_storage_cas_result(true, Some(2))
                .expect("assignment reset result"),
        )
        .expect("deliver assignment reset result");
    expect_chat(
        &boundary,
        "solaris-permissions",
        operator_id,
        "Group reset to player.",
    )
    .await;

    run_player_command(
        &boundary,
        player_id,
        &member,
        "perm check solaris.audit.lookup",
    );
    expect_chat(
        &boundary,
        "solaris-permissions",
        player_id,
        "Permission denied.",
    )
    .await;

    stop_standard_pack(boundary, host).await;
}

#[tokio::test]
async fn standard_pack_essentials_saves_and_revisits_a_home() {
    let plugins = tempfile::tempdir().expect("standard pack tempdir");
    copy_standard_pack(plugins.path());
    let (boundary, host) = start_standard_pack(plugins.path(), None).await;

    let player_id = ScriptPlayerId::new(21);
    let homes_key = format!("homes:{}", normalized(PLAYER_UUID));
    let at_home = ScriptPlayerContext::new(PLAYER_UUID, "Homeowner", false, 1.5, 64.0, 2.5);
    let moved_away = ScriptPlayerContext::new(PLAYER_UUID, "Homeowner", false, 100.5, 70.0, -200.5);

    // Saving a home reads the player's durable home set, then commits it.
    run_player_command(&boundary, player_id, &at_home, "sethome base");
    let admitted = next_command(&boundary, "home set storage read").await;
    assert_eq!(admitted.plugin_id(), "solaris-essentials");
    assert!(
        matches!(
            admitted.request(),
            ScriptCommand::PluginStorageGet { request } if request.key() == homes_key
        ),
        "sethome must read the per-player homes key, saw {:?}",
        admitted.request()
    );
    boundary
        .try_enqueue_event(
            admitted
                .plugin_storage_get_result(None, None)
                .expect("empty homes result"),
        )
        .expect("deliver empty homes result");

    let admitted = next_command(&boundary, "home set commit").await;
    assert_eq!(admitted.plugin_id(), "solaris-essentials");
    let saved_home = match admitted.request() {
        ScriptCommand::PluginStorageCompareAndSwap { request } if request.key() == homes_key => {
            assert_eq!(request.expected_version(), None);
            request.value().to_owned()
        }
        other => panic!("home set must commit through storage CAS, saw {other:?}"),
    };
    assert!(
        saved_home.starts_with("v1|base,1.5,64,2.5"),
        "unexpected encoded home value {saved_home:?}"
    );
    boundary
        .try_enqueue_event(
            admitted
                .plugin_storage_cas_result(true, Some(1))
                .expect("home set commit result"),
        )
        .expect("deliver home set commit result");
    expect_chat(&boundary, "solaris-essentials", player_id, "base saved.").await;

    // Revisiting replays the exact committed encoding and teleports back.
    run_player_command(&boundary, player_id, &moved_away, "home base");
    let admitted = next_command(&boundary, "home revisit storage read").await;
    assert_eq!(admitted.plugin_id(), "solaris-essentials");
    assert!(
        matches!(
            admitted.request(),
            ScriptCommand::PluginStorageGet { request } if request.key() == homes_key
        ),
        "home must re-read the durable homes key, saw {:?}",
        admitted.request()
    );
    boundary
        .try_enqueue_event(
            admitted
                .plugin_storage_get_result(Some(&saved_home), Some(1))
                .expect("saved homes result"),
        )
        .expect("deliver saved homes result");

    let admitted = next_command(&boundary, "home revisit teleport").await;
    assert_eq!(admitted.plugin_id(), "solaris-essentials");
    assert!(
        matches!(
            admitted.request(),
            ScriptCommand::TeleportPlayer { request }
                if request.player_id() == player_id
                    && (request.position().x(), request.position().y(), request.position().z())
                        == (1.5, 64.0, 2.5)
        ),
        "home must teleport to the saved coordinates, saw {:?}",
        admitted.request()
    );
    boundary
        .try_enqueue_event(
            admitted
                .player_teleport_result(None)
                .expect("accepted home teleport result"),
        )
        .expect("deliver accepted home teleport result");
    expect_chat(
        &boundary,
        "solaris-essentials",
        player_id,
        "base teleport complete.",
    )
    .await;

    // A committed teleport leaves a /back location at the departure point.
    run_player_command(&boundary, player_id, &moved_away, "back");
    let admitted = next_command(&boundary, "back teleport").await;
    assert_eq!(admitted.plugin_id(), "solaris-essentials");
    assert!(
        matches!(
            admitted.request(),
            ScriptCommand::TeleportPlayer { request }
                if request.player_id() == player_id
                    && (request.position().x(), request.position().y(), request.position().z())
                        == (100.5, 70.0, -200.5)
        ),
        "back must return to the remembered departure point, saw {:?}",
        admitted.request()
    );
    boundary
        .try_enqueue_event(
            admitted
                .player_teleport_result(None)
                .expect("accepted back teleport result"),
        )
        .expect("deliver accepted back teleport result");
    expect_chat(
        &boundary,
        "solaris-essentials",
        player_id,
        "Back teleport complete.",
    )
    .await;

    // Permission gate: warp and spawn definitions are operator-only.
    run_player_command(&boundary, player_id, &moved_away, "setwarp hub");
    expect_chat(
        &boundary,
        "solaris-essentials",
        player_id,
        "Only an operator can change warps.",
    )
    .await;

    stop_standard_pack(boundary, host).await;
}

#[tokio::test]
async fn standard_pack_economy_pays_once_per_transfer_token() {
    let plugins = tempfile::tempdir().expect("standard pack tempdir");
    copy_standard_pack(plugins.path());
    let (boundary, host) = start_standard_pack(plugins.path(), None).await;

    let player_id = ScriptPlayerId::new(31);
    let payer = ScriptPlayerContext::new(PLAYER_UUID, "Payer", false, 0.0, 64.0, 0.0);
    let payer_key = normalized(PLAYER_UUID);
    let payee_key = normalized(PEER_UUID);

    run_player_command(&boundary, player_id, &payer, "money");
    expect_chat(
        &boundary,
        "solaris-economy",
        player_id,
        "Balance: 100 coins.",
    )
    .await;

    // Paying mutates one bounded ledger record under the configured token.
    run_player_command(
        &boundary,
        player_id,
        &payer,
        &format!("pay {PEER_UUID} 40 rent-q1"),
    );
    let expected_payer_first = format!("v1|{payer_key},60;{payee_key},140|{payer_key}:rent-q1");
    let expected_payee_first = format!("v1|{payee_key},140;{payer_key},60|{payer_key}:rent-q1");
    let admitted = next_command(&boundary, "payment ledger commit").await;
    assert_eq!(admitted.plugin_id(), "solaris-economy");
    assert!(
        matches!(
            admitted.request(),
            ScriptCommand::PluginStorageCompareAndSwap { request }
                if request.key() == "ledger-v1"
                    && request.expected_version().is_none()
                    && (request.value() == expected_payer_first
                        || request.value() == expected_payee_first)
        ),
        "payment must commit the bounded ledger and token window, saw {:?}",
        admitted.request()
    );
    boundary
        .try_enqueue_event(
            admitted
                .plugin_storage_cas_result(true, Some(1))
                .expect("payment commit result"),
        )
        .expect("deliver payment commit result");
    expect_chat(&boundary, "solaris-economy", player_id, "Paid 40 coins.").await;

    // The committed token makes the same payment idempotent.
    run_player_command(
        &boundary,
        player_id,
        &payer,
        &format!("pay {PEER_UUID} 40 rent-q1"),
    );
    expect_chat(
        &boundary,
        "solaris-economy",
        player_id,
        "That transfer token was already committed.",
    )
    .await;

    run_player_command(&boundary, player_id, &payer, "money");
    expect_chat(
        &boundary,
        "solaris-economy",
        player_id,
        "Balance: 60 coins.",
    )
    .await;

    // Permission gate: balance administration is operator-only.
    run_player_command(
        &boundary,
        player_id,
        &payer,
        &format!("econadmin set {PEER_UUID} 999"),
    );
    expect_chat(
        &boundary,
        "solaris-economy",
        player_id,
        "Only an operator can administer balances.",
    )
    .await;

    stop_standard_pack(boundary, host).await;
}

#[tokio::test]
async fn standard_pack_towns_claims_a_leader_protected_chunk_across_restarts() {
    let plugins = tempfile::tempdir().expect("standard pack tempdir");
    copy_standard_pack(plugins.path());

    let founder_id = ScriptPlayerId::new(41);
    let stranger_id = ScriptPlayerId::new(42);
    let founder = ScriptPlayerContext::new(PLAYER_UUID, "Founder", false, 32.0, 64.0, -48.0);
    let stranger = ScriptPlayerContext::new(PEER_UUID, "Stranger", false, 32.0, 64.0, -48.0);
    let founder_key = normalized(PLAYER_UUID);
    let created_value = format!("v1|rivertown,{founder_key},{founder_key}:leader,");
    let claimed_value = format!("v1|rivertown,{founder_key},{founder_key}:leader,2:-3");
    let claim_zone_id = "town-rivertown-p2-n3";

    // First boot: empty town store, so no startup zone is replayed.
    let (boundary, host) = start_standard_pack(plugins.path(), None).await;

    run_player_command(&boundary, founder_id, &founder, "town create rivertown");
    let admitted = next_command(&boundary, "town creation commit").await;
    assert_eq!(admitted.plugin_id(), "solaris-towns");
    assert!(
        matches!(
            admitted.request(),
            ScriptCommand::PluginStorageCompareAndSwap { request }
                if request.key() == "towns-v1"
                    && request.expected_version().is_none()
                    && request.value() == created_value
        ),
        "town creation must commit the leader-only town record, saw {:?}",
        admitted.request()
    );
    boundary
        .try_enqueue_event(
            admitted
                .plugin_storage_cas_result(true, Some(1))
                .expect("town creation commit result"),
        )
        .expect("deliver town creation commit result");
    expect_chat(
        &boundary,
        "solaris-towns",
        founder_id,
        "Town rivertown created.",
    )
    .await;

    // Claiming the standing chunk commits the claim and only then registers the
    // leader-protected zone; the player reply waits for the zone result.
    run_player_command(&boundary, founder_id, &founder, "town claim");
    let admitted = next_command(&boundary, "town claim commit").await;
    assert_eq!(admitted.plugin_id(), "solaris-towns");
    assert!(
        matches!(
            admitted.request(),
            ScriptCommand::PluginStorageCompareAndSwap { request }
                if request.key() == "towns-v1"
                    && request.expected_version() == Some(1)
                    && request.value() == claimed_value
        ),
        "claim must compare-and-swap the committed town record, saw {:?}",
        admitted.request()
    );
    boundary
        .try_enqueue_event(
            admitted
                .plugin_storage_cas_result(true, Some(2))
                .expect("town claim commit result"),
        )
        .expect("deliver town claim commit result");

    let admitted = next_command(&boundary, "town claim zone upsert").await;
    assert_eq!(admitted.plugin_id(), "solaris-towns");
    let (claim_target, claim_zone) = admitted
        .into_upsert_zone()
        .expect("consume town claim zone upsert");
    assert_eq!(claim_zone.id(), claim_zone_id);
    assert_eq!(claim_zone.dimension(), "minecraft:overworld");
    assert_eq!(
        (
            claim_zone.minimum().x(),
            claim_zone.minimum().y(),
            claim_zone.minimum().z()
        ),
        (32.0, -64.0, -48.0)
    );
    assert_eq!(
        (
            claim_zone.maximum().x(),
            claim_zone.maximum().y(),
            claim_zone.maximum().z()
        ),
        (47.0, 319.0, -33.0)
    );
    assert!(
        claim_zone
            .protection()
            .is_some_and(|policy| policy.allowed_actor_uuid() == founder_key),
        "API 0.6 zones authorize exactly one UUID, which must be the town leader"
    );
    boundary
        .try_enqueue_event(
            claim_target
                .zone_command_result(claim_zone_id, true)
                .expect("accepted claim zone result"),
        )
        .expect("deliver accepted claim zone result");
    expect_chat(
        &boundary,
        "solaris-towns",
        founder_id,
        "Chunk claimed for rivertown.",
    )
    .await;

    run_player_command(&boundary, founder_id, &founder, "town info");
    expect_chat(
        &boundary,
        "solaris-towns",
        founder_id,
        "rivertown: 1 members, 1 claims.",
    )
    .await;

    // Permission gate: only the town leader manages claims.
    run_player_command(&boundary, stranger_id, &stranger, "town claim");
    expect_chat(
        &boundary,
        "solaris-towns",
        stranger_id,
        "Only the leader can manage claims.",
    )
    .await;

    stop_standard_pack(boundary, host).await;

    // Second boot over the same plugin root replays the exact committed town
    // record: towns re-registers the claim zone before it reports loaded.
    let (boundary, host) = start_standard_pack(plugins.path(), Some((&claimed_value, 2))).await;

    let admitted = next_command(&boundary, "startup claim zone replay").await;
    assert_eq!(admitted.plugin_id(), "solaris-towns");
    let (replay_target, replay_zone) = admitted
        .into_upsert_zone()
        .expect("consume startup claim zone replay");
    assert_eq!(replay_zone.id(), claim_zone_id);
    assert_eq!(replay_zone.dimension(), "minecraft:overworld");
    assert!(
        replay_zone
            .protection()
            .is_some_and(|policy| policy.allowed_actor_uuid() == founder_key)
    );
    boundary
        .try_enqueue_event(
            replay_target
                .zone_command_result(claim_zone_id, true)
                .expect("accepted replayed claim zone result"),
        )
        .expect("deliver accepted replayed claim zone result");

    // Towns only reports loaded once every replayed zone was accepted.
    run_player_command(&boundary, founder_id, &founder, "town info");
    expect_chat(
        &boundary,
        "solaris-towns",
        founder_id,
        "rivertown: 1 members, 1 claims.",
    )
    .await;

    stop_standard_pack(boundary, host).await;
}

#[tokio::test]
async fn standard_pack_audit_records_committed_actions_and_filters_lookups() {
    let plugins = tempfile::tempdir().expect("standard pack tempdir");
    copy_standard_pack(plugins.path());
    let (boundary, host) = start_standard_pack(plugins.path(), None).await;

    let builder_id = ScriptPlayerId::new(51);
    let auditor_id = ScriptPlayerId::new(52);
    let builder_key = normalized(PLAYER_UUID);
    let builder = ScriptPlayerContext::new(PLAYER_UUID, "Builder", false, 8.0, 64.0, -3.0);
    let auditor_near = ScriptPlayerContext::new(OPERATOR_UUID, "Auditor", true, 9.0, 64.0, -3.0);
    let auditor_far = ScriptPlayerContext::new(OPERATOR_UUID, "Auditor", true, 100.0, 64.0, 0.0);

    // Audit stamps records with the latest delivered simulation tick.
    boundary
        .try_enqueue_latest_server_tick(1200)
        .expect("enqueue simulation tick");

    // A committed block placement is appended to the bounded ring.
    boundary
        .try_enqueue_event(
            ScriptEvent::try_player_block_placed_with_context(
                builder_id,
                builder.clone(),
                "minecraft:overworld",
                "minecraft:stone",
                8,
                64,
                -3,
                ScriptGameMode::Survival,
            )
            .expect("build block placement event"),
        )
        .expect("enqueue block placement event");

    let admitted = next_command(&boundary, "audit record commit").await;
    assert_eq!(admitted.plugin_id(), "solaris-audit");
    assert!(
        matches!(
            admitted.request(),
            ScriptCommand::PluginStorageCompareAndSwap { request }
                if request.key() == "actions-v1"
                    && request.expected_version().is_none()
                    && request.value()
                        == format!("v1|1200,place,{builder_key},8,64,-3,minecraft:stone")
        ),
        "the placed block must append one bounded audit record, saw {:?}",
        admitted.request()
    );
    boundary
        .try_enqueue_event(
            admitted
                .plugin_storage_cas_result(true, Some(1))
                .expect("audit record commit result"),
        )
        .expect("deliver audit record commit result");

    run_player_command(&boundary, auditor_id, &auditor_near, "audit 5");
    expect_chat(
        &boundary,
        "solaris-audit",
        auditor_id,
        &format!("t1200 place {builder_key} @ 8,64,-3 minecraft:stone"),
    )
    .await;

    // Spatial filtering: the same record is outside a small radius.
    run_player_command(&boundary, auditor_id, &auditor_far, "audit here 4 5");
    expect_chat(
        &boundary,
        "solaris-audit",
        auditor_id,
        "No matching bounded audit records.",
    )
    .await;

    // Permission gate: audit lookup is operator-only.
    run_player_command(&boundary, builder_id, &builder, "audit 5");
    expect_chat(
        &boundary,
        "solaris-audit",
        builder_id,
        "Only an operator can query audit history.",
    )
    .await;

    stop_standard_pack(boundary, host).await;
}

fn copy_standard_pack(destination_root: &Path) {
    for name in STANDARD_PACK {
        copy_example_plugin(name, destination_root);
    }
}

fn copy_example_plugin(name: &str, destination_root: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/plugins")
        .join(name);
    let destination = destination_root.join(name);
    std::fs::create_dir(&destination).expect("create copied example plugin directory");
    for file in ["plugin.toml", "main.lua"] {
        std::fs::copy(source.join(file), destination.join(file))
            .unwrap_or_else(|error| panic!("copy shipped {name}/{file}: {error}"));
    }
    let config = source.join("config.toml");
    if config.is_file() {
        std::fs::copy(config, destination.join("config.toml"))
            .unwrap_or_else(|error| panic!("copy shipped {name}/config.toml: {error}"));
    }
}

/// Strict-load the pack and satisfy every storage-backed startup read.
///
/// `seeded_towns` lets a test replay an already-committed town record so the
/// towns startup contract has to re-register its protected zones.
async fn start_standard_pack(
    plugins_root: &Path,
    seeded_towns: Option<(&str, u64)>,
) -> (ScriptBoundary, mc_script::LuaHost) {
    let (boundary, host) = mc_script::start_lua_host(
        mc_script::LuaHostConfig::new(plugins_root).strict_discovery(true),
    )
    .expect("strict-load every standard pack plugin");
    assert_eq!(host.loaded_plugins(), STANDARD_PACK.len());

    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .expect("enqueue server start");
    satisfy_startup_reads(&boundary, seeded_towns).await;
    (boundary, host)
}

/// Answer the four `server.started` storage reads the pack performs.
///
/// solaris-essentials subscribes to no startup event, so exactly four reads
/// arrive and each must be attributed to the plugin that owns its key.
async fn satisfy_startup_reads(boundary: &ScriptBoundary, seeded_towns: Option<(&str, u64)>) {
    for _ in 0..4 {
        let admitted = next_command(boundary, "standard pack startup storage read").await;
        let plugin_id = admitted.plugin_id().to_owned();
        let expected_key = match plugin_id.as_str() {
            "solaris-permissions" => "assignments-v1",
            "solaris-economy" => "ledger-v1",
            "solaris-towns" => "towns-v1",
            "solaris-audit" => "actions-v1",
            other => panic!("unexpected standard pack startup command owner {other}"),
        };
        assert!(
            matches!(
                admitted.request(),
                ScriptCommand::PluginStorageGet { request } if request.key() == expected_key
            ),
            "{plugin_id} must read durable key {expected_key} on server start, saw {:?}",
            admitted.request()
        );
        let seeded = if plugin_id == "solaris-towns" {
            seeded_towns
        } else {
            None
        };
        let result = match seeded {
            Some((value, version)) => admitted
                .plugin_storage_get_result(Some(value), Some(version))
                .expect("seeded towns startup storage result"),
            None => admitted
                .plugin_storage_get_result(None, None)
                .expect("empty startup storage result"),
        };
        boundary
            .try_enqueue_event(result)
            .expect("deliver startup storage result");
    }
}

async fn stop_standard_pack(boundary: ScriptBoundary, host: mc_script::LuaHost) {
    drop(boundary);
    let report = tokio::task::spawn_blocking(move || host.join())
        .await
        .expect("Lua host join task")
        .expect("Lua host thread");
    assert_eq!(
        report.enabled_plugins_at_exit(),
        STANDARD_PACK.len(),
        "no standard pack plugin may stop being enabled: {:?}",
        report.disabled_plugins()
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
        "{raw:?} must be owned by a standard pack plugin"
    );
}

async fn expect_chat(
    boundary: &ScriptBoundary,
    plugin_id: &str,
    player_id: ScriptPlayerId,
    expected: &str,
) {
    let admitted = next_command(boundary, &format!("{plugin_id} chat reply")).await;
    assert_eq!(admitted.plugin_id(), plugin_id);
    assert!(
        matches!(
            admitted.request(),
            ScriptCommand::SendChatMessage { player_id: target, message }
                if *target == player_id && message == expected
        ),
        "{plugin_id} must reply {expected:?} to the acting player, saw {:?}",
        admitted.request()
    );
}
