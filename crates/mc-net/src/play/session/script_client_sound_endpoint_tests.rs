use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use mc_domain::GameMode;
use mc_plugin_host::{
    DeploymentConfig, DiscoveryMode, HostQueues, NoSessions, PluginLimits, discover,
    start_deployment,
};
use mc_script::{ScriptEvent, ScriptPlayerContext, ScriptPlayerId};
use tokio::sync::mpsc;
use uuid::Uuid;

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

use super::script_client_sound_endpoint::ScriptClientSoundRouteError;
use super::{SessionRegistration, SessionRegistry};
use crate::login::LoggedInProfile;
use crate::play::PlayerPose;
use crate::{
    LOADER_PROTOCOL_VERSION, LoaderBundle, LoaderClientAck, LoaderContentKind, LoaderManifest,
    LoaderPermission, LoaderPlatform,
};

#[tokio::test]
async fn sound_commands_reach_only_the_component_owner_session() {
    let manifest = LoaderManifest {
        protocol: LOADER_PROTOCOL_VERSION,
        bundles: vec![LoaderBundle {
            owner: "ruby-live".to_owned(),
            id: "sound".to_owned(),
            version: "1".to_owned(),
            artifact: "client/sound.zip".to_owned(),
            sha256: "a".repeat(64),
            size_bytes: 1,
            loaders: vec![LoaderPlatform::Fabric],
            content: vec![LoaderContentKind::Sounds],
            permissions: vec![LoaderPermission::PlaySounds],
            cache_key: format!("ruby-live:sound/1/{}", "a".repeat(64)),
            view_kinds: Vec::new(),
            source_path: None,
            artifact_bytes: None,
            block_id: None,
            block_name: None,
        }],
    };
    let loader_session = manifest
        .bind_ack(&LoaderClientAck {
            protocol: LOADER_PROTOCOL_VERSION,
            platform: LoaderPlatform::Fabric,
            loader_version: "test".to_owned(),
            accepted_permissions: vec![LoaderPermission::PlaySounds],
            cached_bundles: vec![manifest.bundles[0].cache_key.clone()],
            carrier_block_state_ids: BTreeMap::new(),
        })
        .expect("loader acknowledgement");
    let registry = SessionRegistry::new();
    let (tx, mut rx) = mpsc::channel(4);
    let (session, _) = registry
        .try_register(SessionRegistration {
            profile: &LoggedInProfile {
                uuid: Uuid::from_u128(1),
                name: "Eligible".to_owned(),
            },
            properties: &[],
            center: (0, 0),
            view_distance: 2,
            desired: HashSet::new(),
            tx,
            pose: PlayerPose::new(0.5, 64.0, 0.5),
            game_mode: GameMode::Survival,
            max_sessions: usize::MAX,
            script_operator: false,
            dimension: "minecraft:overworld",
            loader_session: Some(loader_session),
        })
        .expect("session registers");

    let root = tempfile::tempdir().expect("deployment root");
    let package = root.path().join("ruby-live");
    std::fs::create_dir(&package).expect("package directory");
    std::fs::write(
        package.join("plugin.toml"),
        "id = \"ruby-live\"\nname = \"Ruby\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\nplayer_commands = [\"loader_ruby\"]\n",
    )
    .expect("manifest");
    std::fs::write(package.join("config.toml"), "mode = \"loader-live\"\n").expect("config");
    std::fs::write(package.join("plugin.wasm"), hello_component_bytes())
        .expect("component artifact");
    let host = start_deployment(
        discover(
            &DeploymentConfig {
                root: root.path().to_path_buf(),
                mode: DiscoveryMode::Strict,
                expected: vec!["ruby-live".to_owned()],
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
        .try_enqueue_event(
            ScriptEvent::try_player_command_with_context(
                "ruby-live",
                ScriptPlayerId::new(session),
                ScriptPlayerContext::try_new(
                    "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                    "Eligible",
                    false,
                    0.5,
                    64.0,
                    0.5,
                )
                .expect("context"),
                "loader_ruby",
                "sound",
            )
            .expect("component command event"),
        )
        .expect("command queues");
    let setup = boundary.recv_command().await.expect("input setup command");
    boundary
        .accept_host_command(setup)
        .expect("input setup is admitted");
    let sound = boundary.recv_command().await.expect("sound command");
    let admitted = boundary
        .accept_host_command(sound)
        .expect("sound is admitted");
    registry
        .route_script_client_sound_command(admitted, Some(&manifest))
        .expect("owner routes its live session sound");
    assert!(rx.recv().await.is_some());

    boundary
        .try_enqueue_event(
            ScriptEvent::try_player_command_with_context(
                "ruby-live",
                ScriptPlayerId::new(session),
                ScriptPlayerContext::try_new(
                    "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                    "Eligible",
                    false,
                    0.5,
                    64.0,
                    0.5,
                )
                .expect("context"),
                "loader_ruby",
                "sound_foreign_stop",
            )
            .expect("component command event"),
        )
        .expect("command queues");
    let setup = boundary.recv_command().await.expect("input setup command");
    boundary
        .accept_host_command(setup)
        .expect("input setup is admitted");
    let foreign = boundary.recv_command().await.expect("foreign stop command");
    let admitted = boundary
        .accept_host_command(foreign)
        .expect("foreign stop is admitted");
    assert_eq!(
        registry.route_script_client_sound_command(admitted, Some(&manifest)),
        Err(ScriptClientSoundRouteError::SoundNotOwned)
    );
    host.stop();
}
