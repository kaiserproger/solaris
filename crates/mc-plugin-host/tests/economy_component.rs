//! First-party economy component acceptance.
//!
//! The guest source is built and encoded as a real component, then exercised
//! through the same host boundary that admits a deployed package's commands.

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
const TARGET_UUID: &str = "bbbbbbbb-cccc-dddd-eeee-ffffffffffff";
const COMMAND_WAIT: Duration = Duration::from_secs(10);

struct Sessions;

impl PlayerSessions for Sessions {
    fn session_of(&self, player: &str) -> Option<u64> {
        match player {
            PLAYER_UUID => Some(7),
            TARGET_UUID => Some(8),
            _ => None,
        }
    }
}

#[tokio::test]
async fn economy_component_commits_transfers_and_refuses_non_operators() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path());
    let config = DeploymentConfig {
        root: root.path().to_path_buf(),
        mode: DiscoveryMode::Strict,
        expected: vec!["solaris-economy".to_owned()],
        grants: BTreeMap::from([("solaris-economy".to_owned(), vec!["storage".to_owned()])]),
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

    let initial_read = next_command(&boundary, "economy startup storage read").await;
    assert_storage_get(&initial_read, "ledger-v1");
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
        &format!("econadmin set {TARGET_UUID} 500"),
    );
    assert_chat(
        next_command(&boundary, "non-operator economy refusal").await,
        ScriptPlayerId::new(7),
        "Only an operator can administer balances.",
    );

    run_command(
        &boundary,
        ScriptPlayerId::new(7),
        &member,
        &format!("pay {TARGET_UUID} 25 receipt"),
    );
    let transfer = next_command(&boundary, "economy transfer compare-and-swap").await;
    assert!(
        matches!(
            transfer.request(),
            ScriptCommand::PluginStorageCompareAndSwap { request }
                if request.key() == "ledger-v1"
                    && request.expected_version().is_none()
                    && request.value()
                        == "v1|aaaaaaaabbbbccccddddeeeeeeeeeeee,75;bbbbbbbbccccddddeeeeffffffffffff,125|aaaaaaaabbbbccccddddeeeeeeeeeeee:receipt"
        ),
        "economy component must preserve the standard-pack ledger encoding, saw {:?}",
        transfer.request()
    );
    boundary
        .try_enqueue_event(
            transfer
                .plugin_storage_cas_result(true, Some(1))
                .expect("transfer storage result"),
        )
        .expect("deliver transfer storage result");
    assert_chat(
        next_command(&boundary, "transfer success").await,
        ScriptPlayerId::new(7),
        "Paid 25 coins.",
    );

    run_command(&boundary, ScriptPlayerId::new(7), &member, "money");
    assert_chat(
        next_command(&boundary, "post-transfer balance").await,
        ScriptPlayerId::new(7),
        "Balance: 75 coins.",
    );

    let counters = host.stop();
    assert_eq!(counters.len(), 1);
    assert_eq!(counters[0].0, "solaris-economy");
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

fn economy_package_root() -> PathBuf {
    let package = guest_workspace_root().join("packages/solaris-economy");
    assert!(
        package.is_dir(),
        "first-party economy component missing at {}",
        package.display()
    );
    package
}

fn economy_component() -> Vec<u8> {
    fixture::component_bytes_from_workspace(
        &guest_workspace_root(),
        "solaris-economy-plugin",
        "solaris_economy_plugin.wasm",
    )
}

fn write_package(root: &Path) {
    let source = economy_package_root();
    let package = root.join("solaris-economy");
    std::fs::create_dir(&package).expect("package directory");
    for file in ["plugin.toml", "config.toml"] {
        std::fs::copy(source.join(file), package.join(file))
            .unwrap_or_else(|error| panic!("copy first-party economy {file}: {error}"));
    }
    std::fs::write(package.join("plugin.wasm"), economy_component()).expect("component artifact");
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
        "{command:?} must be owned by the economy component"
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
    assert_eq!(command.plugin_id(), "solaris-economy");
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
