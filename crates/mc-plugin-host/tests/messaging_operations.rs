//! Mixed messaging crosses the ordinary admission boundary, atomically.

mod fixture;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use mc_plugin_host::{
    DeploymentConfig, DiscoveryMode, HostQueues, PlayerSessions, PluginHost, PluginLimits,
    start_deployment,
};
use mc_script::{ScriptBoundary, ScriptCommand, ScriptEvent, ScriptPlayerContext, ScriptPlayerId};

const PLAYER: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

struct ReconnectedPlayer;

impl PlayerSessions for ReconnectedPlayer {
    fn session_of(&self, player: &str) -> Option<u64> {
        (player == PLAYER).then_some(11)
    }
}

fn start(reason_bytes: usize, commands: usize) -> (tempfile::TempDir, PluginHost) {
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("hello");
    std::fs::create_dir(&package).unwrap();
    std::fs::write(
        package.join("plugin.toml"),
        "id = \"hello\"\nname = \"Hello\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\nevents = [\"player.joined\"]\n",
    )
    .unwrap();
    std::fs::write(package.join("plugin.wasm"), fixture::component_bytes()).unwrap();
    std::fs::write(
        package.join("config.toml"),
        format!("mode = \"messaging\"\nsize = {reason_bytes}\n"),
    )
    .unwrap();
    let limits = PluginLimits::default();
    let packages = mc_plugin_host::discover(
        &DeploymentConfig {
            root: root.path().to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: vec!["hello".to_owned()],
            grants: BTreeMap::new(),
            require_grants: true,
            precommit_hooks: Vec::new(),
        },
        &limits,
    )
    .unwrap()
    .into_packages();
    let host = start_deployment(
        packages,
        limits,
        HostQueues {
            events: 8,
            commands,
        },
        Arc::new(ReconnectedPlayer),
    )
    .unwrap();
    (root, host)
}

fn join(session: u64) -> ScriptEvent {
    ScriptEvent::player_joined_with_context(
        ScriptPlayerId::new(session),
        ScriptPlayerContext::try_new(PLAYER, "Ada", false, 0.0, 64.0, 0.0).unwrap(),
    )
}

async fn drain(boundary: &ScriptBoundary) -> Vec<ScriptCommand> {
    // Closing admission lets the worker drain both callbacks and close its sender.
    // Only that channel closure, never elapsed silence, proves no prefix escaped.
    boundary.close_event_admission();
    let mut commands = Vec::new();
    while let Some(command) = tokio::time::timeout(Duration::from_secs(10), boundary.recv_command())
        .await
        .expect("the host drains the admitted callbacks")
    {
        commands.push(
            boundary
                .accept_host_command(command)
                .expect("every messaging effect requires real host admission")
                .into_request(),
        );
    }
    commands
}

#[tokio::test]
async fn messaging_is_admitted_without_retargeting_an_old_disconnect() {
    let (_root, host) = start(1, 16);
    let boundary = host.boundary().clone();
    boundary.try_enqueue_event(join(7)).unwrap();
    boundary.try_enqueue_event(join(11)).unwrap();
    let commands = drain(&boundary).await;
    host.stop();

    let mut expected = Vec::new();
    for session in [7, 11] {
        expected.extend([
            // Stable identity follows the current connection, even for the old event.
            ScriptCommand::SendChatMessage {
                player_id: ScriptPlayerId::new(11),
                message: "direct".to_owned(),
            },
            ScriptCommand::BroadcastChatMessage {
                message: "broadcast".to_owned(),
            },
            // Disconnect stays attached to the observed session, not the identity.
            ScriptCommand::DisconnectPlayer {
                player_id: ScriptPlayerId::new(session),
                reason: "x".to_owned(),
            },
        ]);
    }
    assert_eq!(commands, expected);
}

#[tokio::test]
async fn an_invalid_disconnect_discards_the_valid_messaging_prefix_and_retires_the_guest() {
    let (_root, host) = start(mc_script::MAX_SCRIPT_DISCONNECT_REASON_BYTES + 1, 16);
    let boundary = host.boundary().clone();
    boundary.try_enqueue_event(join(7)).unwrap();
    boundary.try_enqueue_event(join(11)).unwrap();
    assert!(drain(&boundary).await.is_empty());
    let diagnostics = host.stop();
    assert_eq!(diagnostics[0].1.events_delivered, 1);
    assert_eq!(diagnostics[0].1.commands_refused, 1);
}

#[tokio::test]
async fn backpressure_discards_every_messaging_effect_without_retiring_the_guest() {
    let (_root, host) = start(1, 2);
    let boundary = host.boundary().clone();
    boundary.try_enqueue_event(join(7)).unwrap();
    boundary.try_enqueue_event(join(11)).unwrap();
    assert!(drain(&boundary).await.is_empty());
    let diagnostics = host.stop();
    assert_eq!(diagnostics[0].1.commands_refused, 6);
}
