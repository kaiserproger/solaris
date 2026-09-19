//! First-party permissions component acceptance.
//!
//! The guest source lives beside the Rust SDK. This test builds that source,
//! encodes its real core module as a component, strict loads its package, and
//! drives the durable operator-only assignment flow through the ordinary host
//! boundary.

mod fixture;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mc_plugin_host::{
    DeploymentConfig, DiscoveryMode, HostQueues, PlayerSessions, PluginLimits, start_deployment,
};
use mc_script::{PlayerCommandAdmission, ScriptCommand, ScriptPlayerContext, ScriptPlayerId};

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
async fn permissions_component_preserves_operator_assignments_and_storage_encoding() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path());
    let config = DeploymentConfig {
        root: root.path().to_path_buf(),
        mode: DiscoveryMode::Strict,
        expected: vec!["solaris-permissions".to_owned()],
        grants: BTreeMap::from([("solaris-permissions".to_owned(), vec!["storage".to_owned()])]),
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

    let initial_read = next_command(&boundary, "permissions startup storage read").await;
    assert_storage_get(&initial_read, "assignments-v1");
    boundary
        .try_enqueue_event(
            initial_read
                .plugin_storage_get_result(None, None)
                .expect("startup storage result"),
        )
        .expect("deliver startup storage result");

    let member = ScriptPlayerContext::new(PLAYER_UUID, "Member", false, 0.0, 64.0, 0.0);
    run_command(
        &boundary,
        ScriptPlayerId::new(7),
        &member,
        "perm user me set moderator",
    );
    assert_chat(
        next_command(&boundary, "non-operator refusal").await,
        ScriptPlayerId::new(7),
        "Only an operator can change groups.",
    );

    let operator = ScriptPlayerContext::new(OPERATOR_UUID, "Operator", true, 0.0, 64.0, 0.0);
    run_command(
        &boundary,
        ScriptPlayerId::new(8),
        &operator,
        &format!("perm user {PLAYER_UUID} set moderator"),
    );
    let assignment = next_command(&boundary, "permission assignment compare-and-swap").await;
    assert!(
        matches!(
            assignment.request(),
            ScriptCommand::PluginStorageCompareAndSwap { request }
                if request.key() == "assignments-v1"
                    && request.expected_version().is_none()
                    && request.value() == "v1|aaaaaaaabbbbccccddddeeeeeeeeeeee,moderator"
        ),
        "permission component must preserve the standard-pack storage encoding, saw {:?}",
        assignment.request()
    );
    boundary
        .try_enqueue_event(
            assignment
                .plugin_storage_cas_result(true, Some(1))
                .expect("assignment storage result"),
        )
        .expect("deliver assignment storage result");
    assert_chat(
        next_command(&boundary, "assignment success").await,
        ScriptPlayerId::new(8),
        "Group set to moderator.",
    );

    run_command(
        &boundary,
        ScriptPlayerId::new(7),
        &member,
        "perm check solaris.audit.lookup",
    );
    assert_chat(
        next_command(&boundary, "promoted permission check").await,
        ScriptPlayerId::new(7),
        "Permission granted.",
    );

    let counters = host.stop();
    assert_eq!(counters.len(), 1);
    assert_eq!(counters[0].0, "solaris-permissions");
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

fn permissions_package_root() -> PathBuf {
    let package = guest_workspace_root().join("packages/solaris-permissions");
    assert!(
        package.is_dir(),
        "first-party permissions component missing at {}",
        package.display()
    );
    package
}

fn permissions_component() -> Vec<u8> {
    fixture::component_bytes_from_workspace(
        &guest_workspace_root(),
        "solaris-permissions-plugin",
        "solaris_permissions_plugin.wasm",
    )
}

fn write_package(root: &Path) {
    let source = permissions_package_root();
    let package = root.join("solaris-permissions");
    std::fs::create_dir(&package).expect("package directory");
    for file in ["plugin.toml", "config.toml"] {
        std::fs::copy(source.join(file), package.join(file))
            .unwrap_or_else(|error| panic!("copy first-party permissions {file}: {error}"));
    }
    std::fs::write(package.join("plugin.wasm"), permissions_component())
        .expect("component artifact");
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
        "{command:?} must be owned by the permissions component"
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
    assert_eq!(command.plugin_id(), "solaris-permissions");
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
