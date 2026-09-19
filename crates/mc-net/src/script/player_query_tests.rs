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
    ScriptPlayerContext, ScriptPlayerId, ScriptPluginManifest,
};

use super::player_query::{PlayerQueryAdapterError, PluginPlayerQueryAdapter};
use crate::play::SessionRegistry;
use crate::server::ScriptEventSink;

fn hello_component_bytes() -> Vec<u8> {
    static BYTES: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(|| {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repository root");
        let sdk = root.join("sdk/rust");
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

fn deployment(root: &Path) -> Vec<mc_plugin_host::LoadedPackage> {
    let package = root.join("who");
    std::fs::create_dir_all(&package).expect("package directory");
    std::fs::write(
        package.join("plugin.toml"),
        "id = \"who\"\nname = \"Who\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\nevents = [\"player.joined\"]\ncapabilities = [\"player_queries\"]\n",
    )
    .expect("manifest");
    std::fs::write(package.join("config.toml"), "mode = \"players\"\n").expect("config");
    std::fs::write(package.join("plugin.wasm"), hello_component_bytes())
        .expect("component artifact");
    discover(
        &DeploymentConfig {
            root: root.to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: vec!["who".to_owned()],
            grants: BTreeMap::new(),
            require_grants: false,
            precommit_hooks: Vec::new(),
        },
        &PluginLimits::default(),
    )
    .expect("strict component deployment discovers")
    .into_packages()
}

async fn admitted_query() -> AdmittedScriptCommand {
    let plugins = tempfile::tempdir().expect("temporary deployment root");
    let host = start_deployment(
        deployment(plugins.path()),
        PluginLimits::default(),
        HostQueues::default(),
        Arc::new(NoSessions),
    )
    .expect("component host starts");
    let boundary = host.boundary().clone();
    boundary
        .try_enqueue_event(ScriptEvent::player_joined_with_context(
            ScriptPlayerId::new(7),
            ScriptPlayerContext::try_new(
                "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                "Ada",
                false,
                0.0,
                64.0,
                0.0,
            )
            .expect("context"),
        ))
        .expect("join event queues");
    let admitted = boundary
        .accept_host_command(boundary.recv_command().await.expect("query command"))
        .expect("component query is admitted");
    host.stop();
    admitted
}

#[tokio::test]
async fn player_query_adapter_publishes_authoritative_targeted_snapshot() {
    let registry = SessionRegistry::new();
    let (boundary, mut events) = mc_script::script_boundary_pair(
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    let manifest = ScriptPluginManifest::new("who", "Who", "0.1.0", COMPONENT_PLUGIN_API_VERSION)
        .declare_capability("player_queries")
        .expect("known component capability")
        .validate()
        .expect("component manifest validates");
    events
        .register_plugin_routes(&manifest)
        .expect("component route registers");
    let adapter = PluginPlayerQueryAdapter::new(ScriptEventSink::new(boundary));
    assert_eq!(
        adapter
            .route_admitted(admitted_query().await, &registry)
            .await,
        Ok(())
    );
    assert!(matches!(
        events.recv_event().await.unwrap().kind(),
        ScriptEventKind::OnlinePlayersResult { request_id, players, truncated }
            if request_id == "who"
                && players.is_empty()
                && !truncated
    ));
}

#[tokio::test]
async fn player_query_adapter_reports_closed_targeted_delivery() {
    let registry = SessionRegistry::new();
    let (boundary, events) = mc_script::script_boundary_pair(
        NonZeroUsize::new(1).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    let adapter = PluginPlayerQueryAdapter::new(ScriptEventSink::new(boundary));
    drop(events);

    assert_eq!(
        adapter
            .route_admitted(admitted_query().await, &registry)
            .await,
        Err(PlayerQueryAdapterError::PublicationClosed)
    );
}
