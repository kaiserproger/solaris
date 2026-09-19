//! The two startup phases of one component deployment, with a real component.
//!
//! `configure` answers the startup contribution from a store of its own that the
//! host drops before the runtime store exists, and `init` then runs exactly once
//! in the runtime store. The fixture's `configure-isolation` mode probes that from
//! the guest's own side: it answers a contribution the host accepts *and* writes a
//! marker down, so a deployment that reports the contribution proves the phase
//! ran, and the line its `init` answers says whether what it wrote reached the
//! runtime store. `mode = "refuse-configure"` is the other half - a package whose
//! own startup phase fails - which may not cost the deployment anything beyond
//! the refusal itself.

mod fixture;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use fixture::component_bytes;
use mc_plugin_host::{
    CheckError, DeploymentConfig, DiscoveryMode, HostQueues, HostStartError, PlayerSessions,
    PluginLimits, check_deployment, start_deployment,
};
use mc_script::{ScriptBoundary, ScriptCommand, ScriptPlayerId, SpawnPlacement};

/// The identity the fixture's probe line is addressed to.
const PROBE_PLAYER: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

/// The session that identity holds in this test's registry.
const PROBE_SESSION: u64 = 7;

/// The number the probe mode writes down while answering the startup
/// contribution, and the marker its `init` would report if the two phases shared
/// a store.
const PROBE_COUNT: usize = 7;

/// The session registry a live server owns: this test knows one player.
struct Sessions;

impl PlayerSessions for Sessions {
    fn session_of(&self, player: &str) -> Option<u64> {
        (player == PROBE_PLAYER).then_some(PROBE_SESSION)
    }
}

fn write_package(root: &Path, id: &str, config: &str) {
    let directory = root.join(id);
    std::fs::create_dir_all(&directory).expect("package directory");
    std::fs::write(
        directory.join("plugin.toml"),
        format!("id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\n"),
    )
    .expect("manifest");
    std::fs::write(directory.join("plugin.wasm"), component_bytes()).expect("artifact");
    std::fs::write(directory.join("config.toml"), config).expect("config");
}

fn deployment(root: &Path, expected: &[&str]) -> DeploymentConfig {
    DeploymentConfig {
        root: root.to_path_buf(),
        mode: DiscoveryMode::Strict,
        expected: expected.iter().map(|id| (*id).to_owned()).collect(),
        grants: BTreeMap::new(),
        require_grants: false,
        precommit_hooks: Vec::new(),
    }
}

/// The next command the boundary admitted, or a panic when none arrives.
async fn admitted(boundary: &ScriptBoundary) -> ScriptCommand {
    tokio::time::timeout(Duration::from_secs(10), boundary.recv_command())
        .await
        .expect("the host answers within the wait")
        .expect("the host answers")
}

#[tokio::test]
async fn a_configure_probe_cannot_reach_the_runtime_store() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "hello",
        &format!("mode = \"configure-isolation\"\ncount = {PROBE_COUNT}\n"),
    );
    let limits = PluginLimits::default();
    let packages = mc_plugin_host::discover(&deployment(root.path(), &["hello"]), &limits)
        .expect("deployment")
        .into_packages();
    let host = start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions))
        .expect("host starts");
    let boundary = host.boundary().clone();

    // The startup phase ran: the contribution it answered is the one the
    // deployment recorded, and it is the fixture's placement category.
    assert!(host.contribution().refusal().is_none());
    let (id, rules) = host
        .contribution()
        .rules()
        .next()
        .expect("the startup phase answered a contribution");
    assert_eq!(id, "hello");
    assert_eq!(
        rules.placement,
        Some(SpawnPlacement::new(2, 8, 4)),
        "the recorded rules are the contribution the startup phase answered"
    );

    // The runtime phase's one line, and this mode's whole answer: what the startup
    // phase wrote into *its* store. A host that ran both phases in one store would
    // report the marker here instead.
    let command = admitted(&boundary).await;
    let ScriptCommand::HostAttached { request, .. } = command else {
        panic!("expected one admitted command, saw {command:?}");
    };
    match request.as_ref() {
        ScriptCommand::SendChatMessage { player_id, message } => {
            assert_eq!(*player_id, ScriptPlayerId::new(PROBE_SESSION));
            assert_eq!(
                message, "configure-state absent",
                "no state the startup phase wrote may exist in the runtime store"
            );
        }
        other => panic!("expected the probe's chat line, saw {other:?}"),
    }

    // `init` runs once per instance: a second run would answer the same line
    // again, and nothing else in this deployment answers one.
    assert!(
        tokio::time::timeout(Duration::from_millis(250), boundary.recv_command())
            .await
            .is_err(),
        "the runtime phase stages exactly one batch"
    );

    let counters = host.stop();
    assert_eq!(counters.len(), 1, "one instance ran");
    assert_eq!(
        counters[0].1.commands_submitted, 1,
        "the one command the runtime phase staged was admitted once"
    );
}

#[test]
fn a_check_runs_both_phases_and_applies_neither() {
    // The check has no session registry and drains no queue, so it verifies the
    // runtime phase's own command instead of applying it - and it reports a
    // command naming a player it cannot resolve as unverifiable rather than
    // inventing a session for it.
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "hello",
        &format!("mode = \"configure-isolation\"\ncount = {PROBE_COUNT}\n"),
    );
    let report = check_deployment(
        &deployment(root.path(), &["hello"]),
        &PluginLimits::default(),
    )
    .expect("the deployment checks");
    let checked = report
        .checked()
        .first()
        .expect("the check reports the one package");
    assert!(
        checked.plan.is_some(),
        "the check ran the startup phase: its contribution validates"
    );
    assert_eq!(
        checked.admitted_commands, 0,
        "the check applies nothing, however much the runtime phase staged"
    );
    assert!(
        checked.unverifiable_player_commands,
        "the command the runtime phase staged names a player a check cannot resolve"
    );
}

#[test]
fn a_failing_startup_phase_refuses_the_check_and_the_deployment() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path(), "hello", "mode = \"refuse-configure\"\n");
    let configuration = deployment(root.path(), &["hello"]);

    let error = check_deployment(&configuration, &PluginLimits::default())
        .expect_err("a package whose startup phase fails cannot be checked in");
    let CheckError::Package { id, .. } = error else {
        panic!("the check reports a package refusal, saw {error:?}");
    };
    assert_eq!(id, "hello");

    let limits = PluginLimits::default();
    let packages = mc_plugin_host::discover(&configuration, &limits)
        .expect("deployment")
        .into_packages();
    let Err(error) = start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions))
    else {
        panic!("a deployment whose startup phase fails must not start");
    };
    let HostStartError::Package { id, .. } = error else {
        panic!("the host reports a package refusal, saw {error:?}");
    };
    assert_eq!(id, "hello");
}
