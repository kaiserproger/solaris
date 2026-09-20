use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use mc_plugin_host::{
    DeploymentConfig, DiscoveryMode, HostQueues, NoSessions, PluginLimits, discover,
    start_deployment,
};
use mc_script::{
    AdmittedScriptCommand, COMPONENT_PLUGIN_API_VERSION, ScriptEvent, ScriptEventKind,
    ScriptPlayerContext, ScriptPlayerId, ScriptPlayerTeleportFailure, ScriptPluginManifest,
};

use super::teleport::PluginTeleportAdapter;
use crate::server::ScriptEventSink;

fn hello_component_bytes() -> Vec<u8> {
    static BYTES: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repository root");
        let sdk = root.join("sdk/rust");
        let _guest_build = crate::test_support::guest_build_lock();
        let status = Command::new(env!("CARGO"))
            .args([
                "build",
                "--manifest-path",
                sdk.join("Cargo.toml")
                    .to_str()
                    .expect("utf-8 workspace path"),
                "--target",
                "wasm32-unknown-unknown",
                "--release",
                "-p",
                "solaris-hello-plugin",
            ])
            .status()
            .expect("guest build starts");
        assert!(status.success(), "hello guest builds");
        let module = std::fs::read(
            sdk.join("target/wasm32-unknown-unknown/release/solaris_hello_plugin.wasm"),
        )
        .expect("guest module exists");
        wit_component::ComponentEncoder::default()
            .module(&module)
            .expect("guest module carries component types")
            .validate(true)
            .encode()
            .expect("guest module encodes as a component")
    });
    BYTES.clone()
}

async fn admitted_teleport_command() -> AdmittedScriptCommand {
    let root = tempfile::tempdir().expect("temporary deployment root");
    let package = root.path().join("warps");
    std::fs::create_dir(&package).expect("package directory");
    std::fs::write(
        package.join("plugin.toml"),
        "id = \"warps\"\nname = \"Teleport test\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\nevents = [\"player.joined\"]\ncapabilities = [\"player_teleport\"]\n",
    )
    .expect("manifest");
    std::fs::write(package.join("config.toml"), "mode = \"teleport\"\n").expect("config");
    std::fs::write(package.join("plugin.wasm"), hello_component_bytes())
        .expect("component artifact");
    let host = start_deployment(
        discover(
            &DeploymentConfig {
                root: root.path().to_path_buf(),
                mode: DiscoveryMode::Strict,
                expected: vec!["warps".to_owned()],
                grants: BTreeMap::new(),
                require_grants: false,
                precommit_hooks: Vec::new(),
            },
            &PluginLimits::default(),
        )
        .expect("strict component deployment discovers")
        .into_packages(),
        PluginLimits::default(),
        HostQueues::default(),
        Arc::new(NoSessions),
    )
    .expect("component host starts");
    let boundary = host.boundary().clone();
    boundary
        .try_enqueue_event(ScriptEvent::player_joined_with_context(
            ScriptPlayerId::new(77),
            ScriptPlayerContext::try_new(
                "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                "Offline",
                false,
                0.0,
                64.0,
                0.0,
            )
            .expect("context"),
        ))
        .expect("join event queues");
    let admitted = boundary
        .accept_host_command(boundary.recv_command().await.expect("teleport command"))
        .expect("component teleport command is admitted");
    host.stop();
    admitted
}

#[tokio::test]
async fn teleport_adapter_publishes_exact_unavailable_result() {
    let command = admitted_teleport_command().await;
    let (boundary, mut events) = mc_script::script_boundary_pair(
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    let manifest = ScriptPluginManifest::new(
        "warps",
        "Teleport test",
        "0.1.0",
        COMPONENT_PLUGIN_API_VERSION,
    )
    .declare_capability("player_teleport")
    .expect("known component capability")
    .validate()
    .expect("component manifest validates");
    events
        .register_plugin_routes(&manifest)
        .expect("component route registers");
    let adapter = PluginTeleportAdapter::new(ScriptEventSink::new(boundary));
    let sessions = crate::play::SessionRegistry::new();
    assert_eq!(adapter.route_admitted(command, &sessions).await, Ok(()));
    assert!(matches!(
        events.recv_event().await.unwrap().kind(),
        ScriptEventKind::PlayerTeleportResult {
            request_id,
            failure: Some(ScriptPlayerTeleportFailure::PlayerUnavailable),
            ..
        } if request_id == "warp-home"
    ));
}
