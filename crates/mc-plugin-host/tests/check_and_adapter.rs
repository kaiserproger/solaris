//! `--check` and the WIT -> DTO adapter: the two places where a guest's answer
//! either becomes a server command or is refused without touching a world.

mod fixture;

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::Path;

use fixture::component_bytes;
use mc_plugin_host::bindings::solaris::plugin::commands::{Command, MessageTarget, SendMessage};
use mc_plugin_host::{
    AdapterError, CommandBatch, DeploymentConfig, DiscoveryMode, NoSessions, PlayerSessions,
    PluginLimits, check_deployment, to_script_batch,
};
use mc_script::{CommandCapabilities, ScriptCommand};

fn write_package(root: &Path, id: &str, config: &str) {
    let directory = root.join(id);
    std::fs::create_dir_all(&directory).expect("package directory");
    std::fs::write(
        directory.join("plugin.toml"),
        format!("id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\n"),
    )
    .expect("manifest");
    std::fs::write(directory.join("plugin.wasm"), component_bytes()).expect("artifact");
    if !config.is_empty() {
        std::fs::write(directory.join("config.toml"), config).expect("config");
    }
}

fn deployment(root: &Path, expected: Vec<String>) -> DeploymentConfig {
    DeploymentConfig {
        root: root.to_path_buf(),
        mode: DiscoveryMode::Strict,
        expected,
        grants: BTreeMap::new(),
        require_grants: false,
    }
}

#[test]
fn a_check_validates_the_real_component_without_touching_a_world() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path(), "hello", "");
    let report = check_deployment(
        &deployment(root.path(), vec!["hello".to_owned()]),
        &PluginLimits::default(),
    )
    .expect("the deployment checks");
    assert_eq!(report.checked().len(), 1);
    let checked = &report.checked()[0];
    assert_eq!(checked.id, "hello");
    assert_eq!(checked.api, "0.7.0");
    assert!(
        checked.plan.is_none(),
        "the example plugin plans no worldgen"
    );
    assert_eq!(checked.admitted_commands, 0, "its init stages no command");
    assert!(!checked.unverifiable_player_commands);
    assert!(report.skipped().is_empty());
}

#[test]
fn a_check_refuses_a_package_that_cannot_finish_its_startup() {
    let root = tempfile::tempdir().expect("deployment root");
    // `mode = "spin"` makes the guest's init never return: a check must report
    // that as a refusal instead of hanging.
    write_package(root.path(), "hello", "mode = \"spin\"\n");
    let error = check_deployment(
        &deployment(root.path(), vec!["hello".to_owned()]),
        &PluginLimits::default(),
    )
    .expect_err("a package that cannot finish its startup is refused");
    assert!(format!("{error}").contains("hello"), "{error}");
    assert!(
        format!("{error}").contains("budget"),
        "the refusal names the budget, saw {error}"
    );
}

#[test]
fn a_session_target_becomes_a_chat_command() {
    let mut batch = CommandBatch::new();
    batch
        .push(
            Command::SendMessage(SendMessage {
                target: MessageTarget::Session(7),
                text: "hello".to_owned(),
            }),
            &PluginLimits::default(),
        )
        .expect("staged");
    let commands = to_script_batch(
        batch,
        NonZeroUsize::new(4).expect("non-zero"),
        &NoSessions,
        // A chat message needs no grant; the conversion is what the package's
        // own declaration allows and nothing more.
        &CommandCapabilities::none(),
    )
    .expect("a session target needs no lookup");
    assert_eq!(
        commands.commands(),
        [ScriptCommand::SendChatMessage {
            player_id: mc_script::ScriptPlayerId::new(7),
            message: "hello".to_owned(),
        }]
    );
}

/// A resolver that knows exactly one player, as a live server would.
struct OneSession(&'static str, u64);

impl PlayerSessions for OneSession {
    fn session_of(&self, player: &str) -> Option<u64> {
        (player == self.0).then_some(self.1)
    }
}

#[test]
fn a_player_identity_is_resolved_at_admission_and_an_absent_one_is_refused() {
    let staged = |player: &str| {
        let mut batch = CommandBatch::new();
        batch
            .push(
                Command::SendMessage(SendMessage {
                    target: MessageTarget::Player(player.to_owned()),
                    text: "hi".to_owned(),
                }),
                &PluginLimits::default(),
            )
            .expect("staged");
        batch
    };
    let known = to_script_batch(
        staged("ada"),
        NonZeroUsize::new(4).expect("non-zero"),
        &OneSession("ada", 9),
        &CommandCapabilities::none(),
    )
    .expect("a connected player resolves");
    assert_eq!(
        known.commands(),
        [ScriptCommand::SendChatMessage {
            player_id: mc_script::ScriptPlayerId::new(9),
            message: "hi".to_owned(),
        }]
    );

    let absent = to_script_batch(
        staged("grace"),
        NonZeroUsize::new(4).expect("non-zero"),
        &OneSession("ada", 9),
        &CommandCapabilities::none(),
    )
    .expect_err("an offline player is refused rather than addressed by a stale id");
    assert_eq!(absent, AdapterError::UnknownPlayer);
}
