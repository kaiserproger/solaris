//! First-party essentials component acceptance.
//!
//! The test drives durable homes and operator-only warps through the real guest
//! component, the normal boundary, and the host's storage correlation path.

mod fixture;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mc_plugin_host::{
    DeploymentConfig, DiscoveryMode, HostQueues, PlayerSessions, PluginLimits, start_deployment,
};
use mc_script::{PlayerCommandAdmission, ScriptCommand, ScriptPlayerContext, ScriptPlayerId};

const MEMBER_UUID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const OPERATOR_UUID: &str = "11111111-2222-3333-4444-555555555555";
const COMMAND_WAIT: Duration = Duration::from_secs(10);

struct Sessions;

impl PlayerSessions for Sessions {
    fn session_of(&self, player: &str) -> Option<u64> {
        match player {
            MEMBER_UUID => Some(7),
            OPERATOR_UUID => Some(8),
            _ => None,
        }
    }
}

#[tokio::test]
async fn essentials_component_persists_locations_and_enforces_warp_authority() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path());
    let config = DeploymentConfig {
        root: root.path().to_path_buf(),
        mode: DiscoveryMode::Strict,
        expected: vec!["solaris-essentials".to_owned()],
        grants: BTreeMap::from([(
            "solaris-essentials".to_owned(),
            vec![
                "storage".to_owned(),
                "player_teleport".to_owned(),
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

    let member = ScriptPlayerContext::new(MEMBER_UUID, "Member", false, 1.0, 65.0, 3.0);
    run_command(&boundary, ScriptPlayerId::new(7), &member, "sethome base");
    let home_read = next_command(&boundary, "home storage read").await;
    assert_storage_get(&home_read, "homes:aaaaaaaabbbbccccddddeeeeeeeeeeee");
    boundary
        .try_enqueue_event(
            home_read
                .plugin_storage_get_result(None, None)
                .expect("home storage result"),
        )
        .expect("deliver home storage result");
    let home_save = next_command(&boundary, "home storage compare-and-swap").await;
    assert_storage_cas(
        &home_save,
        "homes:aaaaaaaabbbbccccddddeeeeeeeeeeee",
        None,
        "v1|base,1,65,3",
    );
    boundary
        .try_enqueue_event(
            home_save
                .plugin_storage_cas_result(true, Some(1))
                .expect("home storage commit"),
        )
        .expect("deliver home storage commit");
    assert_chat(
        next_command(&boundary, "home save acknowledgement").await,
        ScriptPlayerId::new(7),
        "base saved.",
    );

    run_command(&boundary, ScriptPlayerId::new(7), &member, "setwarp plaza");
    assert_chat(
        next_command(&boundary, "non-operator warp refusal").await,
        ScriptPlayerId::new(7),
        "Only an operator can change warps.",
    );

    let operator = ScriptPlayerContext::new(OPERATOR_UUID, "Operator", true, 4.0, 70.0, -2.0);
    run_command(
        &boundary,
        ScriptPlayerId::new(8),
        &operator,
        "setwarp plaza",
    );
    let warp_read = next_command(&boundary, "warp storage read").await;
    assert_storage_get(&warp_read, "warps-v1");
    boundary
        .try_enqueue_event(
            warp_read
                .plugin_storage_get_result(None, None)
                .expect("warp storage result"),
        )
        .expect("deliver warp storage result");
    let warp_save = next_command(&boundary, "warp storage compare-and-swap").await;
    assert_storage_cas(&warp_save, "warps-v1", None, "v1|plaza,4,70,-2");
    boundary
        .try_enqueue_event(
            warp_save
                .plugin_storage_cas_result(true, Some(1))
                .expect("warp storage commit"),
        )
        .expect("deliver warp storage commit");
    assert_chat(
        next_command(&boundary, "warp save acknowledgement").await,
        ScriptPlayerId::new(8),
        "plaza saved.",
    );

    run_command(
        &boundary,
        ScriptPlayerId::new(7),
        &member,
        "msg Operator   preserving  spaces  ",
    );
    let online_query = next_command(&boundary, "message online-player query").await;
    assert!(
        matches!(
            online_query.request(),
            ScriptCommand::ListOnlinePlayers { .. }
        ),
        "message command must query the authoritative player snapshot"
    );
    let operator_snapshot = mc_script::ScriptOnlinePlayerSnapshot::try_new(
        ScriptPlayerId::new(8),
        operator.clone(),
        "minecraft:overworld",
    )
    .expect("operator snapshot");
    boundary
        .try_enqueue_event(
            online_query
                .online_players_result(vec![operator_snapshot], false)
                .expect("message query result"),
        )
        .expect("deliver message query result");
    assert_chat(
        next_command(&boundary, "message recipient delivery").await,
        ScriptPlayerId::new(8),
        "[from Member] preserving  spaces  ",
    );
    assert_chat(
        next_command(&boundary, "message sender delivery").await,
        ScriptPlayerId::new(7),
        "[to Operator] preserving  spaces  ",
    );

    let counters = host.stop();
    assert_eq!(counters.len(), 1);
    assert_eq!(counters[0].0, "solaris-essentials");
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

fn essentials_package_root() -> PathBuf {
    let package = guest_workspace_root().join("packages/solaris-essentials");
    assert!(
        package.is_dir(),
        "first-party essentials component missing at {}",
        package.display()
    );
    package
}

fn essentials_component() -> Vec<u8> {
    fixture::component_bytes_from_workspace(
        &guest_workspace_root(),
        "solaris-essentials-plugin",
        "solaris_essentials_plugin.wasm",
    )
}

fn write_package(root: &Path) {
    let source = essentials_package_root();
    let package = root.join("solaris-essentials");
    std::fs::create_dir(&package).expect("package directory");
    for file in ["plugin.toml", "config.toml"] {
        std::fs::copy(source.join(file), package.join(file))
            .unwrap_or_else(|error| panic!("copy first-party essentials {file}: {error}"));
    }
    std::fs::write(package.join("plugin.wasm"), essentials_component())
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
        "{command:?} must be owned by the essentials component"
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
    assert_eq!(command.plugin_id(), "solaris-essentials");
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
