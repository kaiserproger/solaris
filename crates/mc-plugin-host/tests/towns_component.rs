//! First-party towns component acceptance.
//!
//! The test builds the deployed guest source, encodes its real core module as a
//! component, and drives the durable town creation flow through the ordinary host
//! boundary. It also proves that a player without town leadership cannot claim.

mod fixture;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mc_plugin_host::{
    DeploymentConfig, DiscoveryMode, HostQueues, PlayerSessions, PluginLimits, start_deployment,
};
use mc_script::{PlayerCommandAdmission, ScriptCommand, ScriptPlayerContext, ScriptPlayerId};

const LEADER_UUID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const MEMBER_UUID: &str = "11111111-2222-3333-4444-555555555555";
const COMMAND_WAIT: Duration = Duration::from_secs(10);

struct Sessions;

impl PlayerSessions for Sessions {
    fn session_of(&self, player: &str) -> Option<u64> {
        match player {
            LEADER_UUID => Some(7),
            MEMBER_UUID => Some(8),
            _ => None,
        }
    }
}

#[tokio::test]
async fn towns_component_persists_creation_and_refuses_non_leader_claims() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path());
    let config = DeploymentConfig {
        root: root.path().to_path_buf(),
        mode: DiscoveryMode::Strict,
        expected: vec!["solaris-towns".to_owned()],
        grants: BTreeMap::from([(
            "solaris-towns".to_owned(),
            vec![
                "storage".to_owned(),
                "zones".to_owned(),
                "player_queries".to_owned(),
            ],
        )]),
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

    let startup = next_command(&boundary, "town startup storage read").await;
    assert_storage_get(&startup, "towns-v1");
    boundary
        .try_enqueue_event(
            startup
                .plugin_storage_get_result(None, None)
                .expect("town startup storage result"),
        )
        .expect("deliver town startup storage result");

    let leader = ScriptPlayerContext::new(LEADER_UUID, "Leader", false, 33.0, 70.0, -17.0);
    run_command(
        &boundary,
        ScriptPlayerId::new(7),
        &leader,
        "town create riverton",
    );
    let creation = next_command(&boundary, "town creation compare-and-swap").await;
    assert_storage_cas(
        &creation,
        "towns-v1",
        None,
        "v1|riverton,aaaaaaaabbbbccccddddeeeeeeeeeeee,aaaaaaaabbbbccccddddeeeeeeeeeeee:leader,",
    );
    boundary
        .try_enqueue_event(
            creation
                .plugin_storage_cas_result(true, Some(1))
                .expect("town creation storage result"),
        )
        .expect("deliver town creation storage result");
    assert_chat(
        next_command(&boundary, "town creation acknowledgement").await,
        ScriptPlayerId::new(7),
        "Town riverton created.",
    );

    let member = ScriptPlayerContext::new(MEMBER_UUID, "Member", false, 33.0, 70.0, -17.0);
    run_command(&boundary, ScriptPlayerId::new(8), &member, "town claim");
    assert_chat(
        next_command(&boundary, "non-leader claim refusal").await,
        ScriptPlayerId::new(8),
        "Only the leader can manage claims.",
    );

    let counters = host.stop();
    assert_eq!(counters.len(), 1);
    assert_eq!(counters[0].0, "solaris-towns");
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

fn towns_package_root() -> PathBuf {
    let package = guest_workspace_root().join("packages/solaris-towns");
    assert!(
        package.is_dir(),
        "first-party towns component missing at {}",
        package.display()
    );
    package
}

fn towns_component() -> Vec<u8> {
    fixture::component_bytes_from_workspace(
        &guest_workspace_root(),
        "solaris-towns-plugin",
        "solaris_towns_plugin.wasm",
    )
}

fn write_package(root: &Path) {
    let source = towns_package_root();
    let package = root.join("solaris-towns");
    std::fs::create_dir(&package).expect("package directory");
    for file in ["plugin.toml", "config.toml"] {
        std::fs::copy(source.join(file), package.join(file))
            .unwrap_or_else(|error| panic!("copy first-party towns {file}: {error}"));
    }
    std::fs::write(package.join("plugin.wasm"), towns_component()).expect("component artifact");
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
        "{command:?} must be owned by the towns component"
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

fn assert_storage_cas(
    command: &mc_script::AdmittedScriptCommand,
    key: &str,
    expected_version: Option<u64>,
    value: &str,
) {
    assert!(
        matches!(
            command.request(),
            ScriptCommand::PluginStorageCompareAndSwap { request }
                if request.key() == key
                    && request.expected_version() == expected_version
                    && request.value() == value
        ),
        "expected storage compare-and-swap for {key:?}, saw {:?}",
        command.request()
    );
}

fn assert_chat(command: mc_script::AdmittedScriptCommand, player: ScriptPlayerId, expected: &str) {
    assert_eq!(command.plugin_id(), "solaris-towns");
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
