//! First-party audit component acceptance.
//!
//! The guest is compiled from the SDK workspace and encoded as a real component.
//! This exercise loads it through the ordinary deployment boundary, commits one
//! audit record durably, and proves the audit root remains operator-only.

mod fixture;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mc_plugin_host::{
    DeploymentConfig, DiscoveryMode, HostQueues, PlayerSessions, PluginLimits, start_deployment,
};
use mc_script::{
    PlayerCommandAdmission, ScriptCommand, ScriptEvent, ScriptGameMode, ScriptPlayerContext,
    ScriptPlayerId,
};

const PLAYER_UUID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const OPERATOR_UUID: &str = "11111111-2222-3333-4444-555555555555";
const COMMAND_WAIT: Duration = Duration::from_secs(10);

struct Sessions;

impl PlayerSessions for Sessions {
    fn session_of(&self, player: &str) -> Option<u64> {
        match player {
            PLAYER_UUID => Some(7),
            OPERATOR_UUID => Some(8),
            _ => None,
        }
    }
}

#[tokio::test]
async fn audit_component_persists_bounded_records_and_enforces_lookup_authority() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path());
    let config = DeploymentConfig {
        root: root.path().to_path_buf(),
        mode: DiscoveryMode::Strict,
        expected: vec!["solaris-audit".to_owned()],
        grants: BTreeMap::from([("solaris-audit".to_owned(), vec!["storage".to_owned()])]),
        require_grants: true,
        precommit_hooks: Vec::new(),
    };
    let limits = PluginLimits::default();
    let packages = mc_plugin_host::discover(&config, &limits)
        .expect("strict component deployment")
        .into_packages();
    let host = start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions))
        .expect("component host starts");
    let boundary = host.boundary().clone();

    let initial_read = next_command(&boundary, "audit startup storage read").await;
    assert_storage_get(&initial_read, "actions-v1");
    boundary
        .try_enqueue_event(
            initial_read
                .plugin_storage_get_result(None, None)
                .expect("startup storage result"),
        )
        .expect("deliver startup storage result");

    let member = ScriptPlayerContext::new(PLAYER_UUID, "Member", false, 12.0, 64.0, -4.0);
    boundary
        .try_enqueue_event(
            ScriptEvent::try_player_block_broken_with_context(
                ScriptPlayerId::new(7),
                member.clone(),
                "minecraft:overworld",
                "minecraft:diamond_ore",
                12,
                64,
                -4,
                ScriptGameMode::Survival,
            )
            .expect("committed block-break event"),
        )
        .expect("deliver block-break event");
    let append = next_command(&boundary, "audit append compare-and-swap").await;
    assert!(
        matches!(
            append.request(),
            ScriptCommand::PluginStorageCompareAndSwap { request }
                if request.key() == "actions-v1"
                    && request.expected_version().is_none()
                    && request.value().starts_with("v1|")
                    && request.value().ends_with(",break,aaaaaaaabbbbccccddddeeeeeeeeeeee,12,64,-4,minecraft:diamond_ore")
        ),
        "audit component must preserve actions-v1 encoding, saw {:?}",
        append.request()
    );
    boundary
        .try_enqueue_event(
            append
                .plugin_storage_cas_result(true, Some(1))
                .expect("audit storage commit"),
        )
        .expect("deliver audit storage commit");

    run_command(&boundary, ScriptPlayerId::new(7), &member, "audit");
    assert_chat(
        next_command(&boundary, "non-operator audit refusal").await,
        ScriptPlayerId::new(7),
        "Only an operator can query audit history.",
    );

    let operator = ScriptPlayerContext::new(OPERATOR_UUID, "Operator", true, 12.0, 64.0, -4.0);
    run_command(
        &boundary,
        ScriptPlayerId::new(8),
        &operator,
        "audit here 0 1",
    );
    let report = next_command(&boundary, "operator audit report").await;
    assert_eq!(report.plugin_id(), "solaris-audit");
    assert!(
        matches!(
            report.request(),
            ScriptCommand::SendChatMessage { player_id, message }
                if *player_id == ScriptPlayerId::new(8)
                    && message.starts_with('t')
                    && message.ends_with(" break aaaaaaaabbbbccccddddeeeeeeeeeeee @ 12,64,-4 minecraft:diamond_ore")
        ),
        "operator must receive the committed audit record, saw {:?}",
        report.request()
    );

    let counters = host.stop();
    assert_eq!(counters.len(), 1);
    assert_eq!(counters[0].0, "solaris-audit");
    assert_eq!(counters[0].1.commands_refused, 0);
}

fn guest_workspace_root() -> PathBuf {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("sdk/rust")
        .canonicalize()
        .expect("guest SDK workspace");
    assert!(
        workspace.is_dir(),
        "guest SDK workspace missing at {}",
        workspace.display()
    );
    workspace
}

fn audit_package_root() -> PathBuf {
    let package = guest_workspace_root().join("packages/solaris-audit");
    assert!(
        package.is_dir(),
        "first-party audit component missing at {}",
        package.display()
    );
    package
}

fn audit_component() -> Vec<u8> {
    fixture::component_bytes_from_workspace(
        &guest_workspace_root(),
        "solaris-audit-plugin",
        "solaris_audit_plugin.wasm",
    )
}

fn write_package(root: &Path) {
    let source = audit_package_root();
    let package = root.join("solaris-audit");
    std::fs::create_dir(&package).expect("package directory");
    for file in ["plugin.toml", "config.toml"] {
        std::fs::copy(source.join(file), package.join(file))
            .unwrap_or_else(|error| panic!("copy first-party audit {file}: {error}"));
    }
    std::fs::write(package.join("plugin.wasm"), audit_component()).expect("component artifact");
}

fn run_command(
    boundary: &mc_script::ScriptBoundary,
    player: ScriptPlayerId,
    context: &ScriptPlayerContext,
    command: &str,
) {
    assert_eq!(
        boundary
            .try_enqueue_player_command_with_context(player, context.clone(), command)
            .expect("enqueue component command"),
        PlayerCommandAdmission::Enqueued,
        "{command:?} must be owned by the audit component"
    );
}

async fn next_command(
    boundary: &mc_script::ScriptBoundary,
    what: &str,
) -> mc_script::AdmittedScriptCommand {
    let command = tokio::time::timeout(COMMAND_WAIT, boundary.recv_command())
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
        .unwrap_or_else(|| panic!("component host closed while waiting for {what}"));
    boundary
        .accept_host_command(command)
        .unwrap_or_else(|error| panic!("admit {what}: {error:?}"))
}

fn assert_storage_get(command: &mc_script::AdmittedScriptCommand, key: &str) {
    assert!(
        matches!(
            command.request(),
            ScriptCommand::PluginStorageGet { request } if request.key() == key
        ),
        "expected storage read for {key:?}, saw {:?}",
        command.request()
    );
}

fn assert_chat(command: mc_script::AdmittedScriptCommand, player: ScriptPlayerId, expected: &str) {
    assert_eq!(command.plugin_id(), "solaris-audit");
    assert!(
        matches!(
            command.request(),
            ScriptCommand::SendChatMessage { player_id, message }
                if *player_id == player && message == expected
        ),
        "expected chat {expected:?}, saw {:?}",
        command.request()
    );
}
