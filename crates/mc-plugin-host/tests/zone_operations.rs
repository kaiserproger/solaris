//! Real components receive zone results only for their own admitted commands.

mod fixture;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use mc_plugin_host::{
    DeploymentConfig, DiscoveryMode, HostQueues, PlayerSessions, PluginHost, PluginLimits,
    start_deployment,
};
use mc_script::{ScriptBoundary, ScriptCommand, ScriptEvent, ScriptPlayerContext, ScriptPlayerId};

const PLAYER_UUID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
const SESSION: u64 = 7;

struct Sessions;

impl PlayerSessions for Sessions {
    fn session_of(&self, player: &str) -> Option<u64> {
        (player == PLAYER_UUID).then_some(SESSION)
    }
}

fn write_package(root: &Path, id: &str, zones: bool) {
    let directory = root.join(id);
    std::fs::create_dir(&directory).unwrap();
    let capabilities = if zones {
        "capabilities = [\"zones\"]\n"
    } else {
        ""
    };
    std::fs::write(
        directory.join("plugin.toml"),
        format!(
            "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\n\
             events = [\"player.joined\", \"zone.command_result\"]\n\
             player_commands = [\"{id}\"]\n{capabilities}"
        ),
    )
    .unwrap();
    std::fs::write(directory.join("plugin.wasm"), fixture::component_bytes()).unwrap();
    std::fs::write(
        directory.join("config.toml"),
        format!("greeting = \"{id}\"\nmode = \"zones\"\n"),
    )
    .unwrap();
}

fn start(root: &Path, ids: &[&str]) -> PluginHost {
    let limits = PluginLimits::default();
    let packages = mc_plugin_host::discover(
        &DeploymentConfig {
            root: root.to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: ids.iter().map(|id| (*id).to_owned()).collect(),
            grants: BTreeMap::new(),
            require_grants: false,
            precommit_hooks: Vec::new(),
        },
        &limits,
    )
    .unwrap()
    .into_packages();
    start_deployment(packages, limits, HostQueues::default(), Arc::new(Sessions)).unwrap()
}

fn join() -> ScriptEvent {
    let player = ScriptPlayerContext::try_new(PLAYER_UUID, "Ada", false, 0.0, 64.0, 0.0).unwrap();
    ScriptEvent::player_joined_with_context(ScriptPlayerId::new(SESSION), player)
}

async fn receive(boundary: &ScriptBoundary) -> Option<ScriptCommand> {
    tokio::time::timeout(Duration::from_secs(5), boundary.recv_command())
        .await
        .expect("the host must deliver its output or close the command queue")
}

#[tokio::test]
async fn zone_results_preserve_the_owner_outcome_and_never_reach_another_package() {
    let root = tempfile::tempdir().unwrap();
    for id in ["hello", "other"] {
        write_package(root.path(), id, true);
    }
    let host = start(root.path(), &["hello", "other"]);
    let boundary = host.boundary().clone();
    boundary.try_enqueue_event(join()).unwrap();

    // Both guests issue the same zone ids. Only the admission target distinguishes
    // their answers; a zone-id-only router would leak one owner's result to both.
    let mut results = Vec::new();
    for _ in 0..6 {
        let command = receive(&boundary).await.expect("guest zone command");
        let admitted = boundary.accept_host_command(command).unwrap();
        let owner = admitted.plugin_id().to_owned();
        let (target, zone_id) = match admitted.request() {
            ScriptCommand::UpsertZone { .. } => {
                let (target, zone) = admitted.into_upsert_zone().unwrap();
                (target, zone.id().to_owned())
            }
            ScriptCommand::RemoveZone { .. } => admitted.into_remove_zone().unwrap(),
            other => panic!("expected an admitted zone command, got {other:?}"),
        };
        // The zone owner publishes only a bit. In particular a refused removal
        // must not disappear or acquire an invented failure reason.
        let accepted = owner == "hello" && zone_id == "market-stall";
        results.push(target.zone_command_result(zone_id, accepted).unwrap());
    }
    for result in results {
        boundary.try_enqueue_event(result).unwrap();
    }
    // Close admission, not the host stop flag: the worker drains every queued
    // result before dropping its command sender. No timed silence proves success.
    boundary.close_event_admission();
    let mut observed = Vec::new();
    while let Some(command) = receive(&boundary).await {
        let ScriptCommand::HostAttached {
            provenance,
            request,
        } = command
        else {
            panic!("expected host provenance");
        };
        let ScriptCommand::SendChatMessage { message, .. } = request.as_ref() else {
            panic!("expected the guest's zone answer");
        };
        observed.push((provenance.plugin_id().to_owned(), message.clone()));
    }
    host.stop();
    observed.sort();
    assert_eq!(
        observed,
        [
            ("hello", "hello claim-1 refused"),
            ("hello", "hello claim-2 refused"),
            ("hello", "hello market-stall applied"),
            ("other", "other claim-1 refused"),
            ("other", "other claim-2 refused"),
            ("other", "other market-stall refused"),
        ]
        .map(|(owner, message)| (owner.to_owned(), message.to_owned()))
    );
}

#[tokio::test]
async fn an_ungranted_zone_batch_publishes_nothing_and_retires_its_package() {
    let root = tempfile::tempdir().unwrap();
    write_package(root.path(), "hello", false);
    let host = start(root.path(), &["hello"]);
    let boundary = host.boundary().clone();
    assert_eq!(boundary.player_command_roots(), ["hello"]);
    boundary.try_enqueue_event(join()).unwrap();
    // A retired guest must not run the next callback. Closing event admission
    // itself clears routes, so route emptiness cannot establish retirement.
    boundary.try_enqueue_event(join()).unwrap();
    boundary.close_event_admission();
    assert!(receive(&boundary).await.is_none());
    let counters = host.stop();
    assert_eq!(counters[0].1.commands_refused, 1);
    assert_eq!(counters[0].1.commands_submitted, 0);
}
