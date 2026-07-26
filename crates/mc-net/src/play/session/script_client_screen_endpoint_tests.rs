use std::collections::{BTreeMap, HashSet};

use mc_script::{LuaHostConfig, ScriptEvent, ScriptPlayerId, start_lua_host};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::loader::{encode_loader_open_screen, loader_open_screen_channel};
use crate::{
    LOADER_PROTOCOL_VERSION, LoaderBundle, LoaderClientAck, LoaderContentKind, LoaderManifest,
    LoaderPermission, LoaderPlatform,
};

use super::outbound::OutboundCommand;
use super::script_client_screen_endpoint::{
    ScriptClientScreenRouteError, plugin_has_screen_bundle, screen_is_owned,
};
use super::{SessionRegistration, SessionRegistry};
use crate::login::LoggedInProfile;
use crate::play::PlayerPose;

fn manifest(content: Vec<LoaderContentKind>, permissions: Vec<LoaderPermission>) -> LoaderManifest {
    LoaderManifest {
        protocol: LOADER_PROTOCOL_VERSION,
        bundles: vec![LoaderBundle {
            owner: "example".to_owned(),
            id: "screen".to_owned(),
            version: "1".to_owned(),
            artifact: "client/screen.zip".to_owned(),
            sha256: "a".repeat(64),
            size_bytes: 1,
            loaders: vec![LoaderPlatform::Fabric],
            content,
            permissions,
            cache_key: format!("example:screen/1/{}", "a".repeat(64)),
            source_path: None,
            block_id: None,
            block_name: None,
        }],
    }
}

#[test]
fn screen_policy_requires_matching_owner_content_and_permission() {
    let eligible = manifest(
        vec![LoaderContentKind::Screens],
        vec![LoaderPermission::OpenScreens],
    );
    assert!(plugin_has_screen_bundle(&eligible, "example"));
    assert!(!plugin_has_screen_bundle(&eligible, "other"));
    assert!(!plugin_has_screen_bundle(
        &manifest(
            vec![LoaderContentKind::Assets],
            vec![LoaderPermission::LoadAssets],
        ),
        "example",
    ));
}

#[test]
fn admitted_plugin_can_only_name_its_own_screen_namespace() {
    assert!(screen_is_owned("example", "example:welcome"));
    assert!(!screen_is_owned("example", "other:welcome"));
    assert!(!screen_is_owned("example", "example:"));
}

fn profile(id: u128, name: &str) -> LoggedInProfile {
    LoggedInProfile {
        uuid: Uuid::from_u128(id),
        name: name.to_owned(),
    }
}

#[tokio::test]
async fn admitted_screen_route_requires_exact_loader_session_and_publishes_raw_payload() {
    let registry = SessionRegistry::new();
    let eligible_profile = profile(1, "Eligible");
    let vanilla_profile = profile(2, "Vanilla");
    let closed_profile = profile(3, "Closed");
    let (eligible_tx, mut eligible_rx) = mpsc::channel(4);
    let (vanilla_tx, _vanilla_rx) = mpsc::channel(4);
    let (closed_tx, closed_rx) = mpsc::channel(4);
    drop(closed_rx);

    let mut loader_manifest = manifest(
        vec![LoaderContentKind::Screens, LoaderContentKind::Blocks],
        vec![
            LoaderPermission::OpenScreens,
            LoaderPermission::RegisterBlocks,
        ],
    );
    loader_manifest.bundles[0].block_id = Some("example:ruby_block".to_owned());
    let loader_session = loader_manifest
        .bind_ack(&LoaderClientAck {
            protocol: LOADER_PROTOCOL_VERSION,
            platform: LoaderPlatform::Fabric,
            loader_version: "test".to_owned(),
            accepted_permissions: loader_manifest.bundles[0].permissions.clone(),
            cached_bundles: vec![loader_manifest.bundles[0].cache_key.clone()],
            carrier_block_state_ids: BTreeMap::from([("example:ruby_block".to_owned(), 321)]),
        })
        .unwrap();
    let registration = |profile, tx, loader_session| SessionRegistration {
        profile,
        properties: &[],
        center: (0, 0),
        view_distance: 2,
        desired: HashSet::new(),
        tx,
        pose: PlayerPose::new(0.5, 64.0, 0.5),
        max_sessions: usize::MAX,
        script_operator: false,
        dimension: "minecraft:overworld",
        loader_session,
    };
    let (eligible_id, _) = registry
        .try_register(registration(
            &eligible_profile,
            eligible_tx,
            Some(loader_session.clone()),
        ))
        .unwrap();
    let (vanilla_id, _) = registry
        .try_register(registration(&vanilla_profile, vanilla_tx, None))
        .unwrap();
    let (closed_id, _) = registry
        .try_register(registration(
            &closed_profile,
            closed_tx,
            Some(loader_session),
        ))
        .unwrap();
    let plugins = tempfile::tempdir().unwrap();
    let plugin = plugins.path().join("example");
    std::fs::create_dir(&plugin).unwrap();
    std::fs::write(
        plugin.join("plugin.toml"),
        r#"id = "example"
name = "Example"
version = "0.1.0"
api = "0.6.0"
events = ["server.started"]
"#,
    )
    .unwrap();
    std::fs::write(
        plugin.join("main.lua"),
        format!(
            r#"
function on_server_started(_event)
    solaris.open_client_screen({eligible_id}, "example:welcome")
    solaris.open_client_screen({vanilla_id}, "example:welcome")
    solaris.open_client_screen({closed_id}, "example:welcome")
    solaris.open_client_screen(999999, "example:welcome")
    solaris.open_client_screen({eligible_id}, "other:welcome")
    solaris.open_client_screen({eligible_id}, "example:welcome")
end
"#
        ),
    )
    .unwrap();
    let (boundary, host) = start_lua_host(LuaHostConfig::new(plugins.path())).unwrap();
    assert_eq!(host.loaded_plugins(), 1);
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let manifest = manifest(
        vec![LoaderContentKind::Screens],
        vec![LoaderPermission::OpenScreens],
    );

    let admitted = boundary
        .accept_host_command(boundary.recv_command().await.unwrap())
        .unwrap();
    registry
        .route_script_client_screen_command(admitted, Some(&manifest))
        .unwrap();
    match eligible_rx.recv().await.unwrap() {
        OutboundCommand::CustomPayload { channel, payload } => {
            assert_eq!(channel, *loader_open_screen_channel());
            assert_eq!(
                payload,
                encode_loader_open_screen("example:welcome").unwrap()
            );
        }
        other => panic!("expected Loader custom payload, got {other:?}"),
    }

    for expected_player in [vanilla_id, closed_id, 999999] {
        let admitted = boundary
            .accept_host_command(boundary.recv_command().await.unwrap())
            .unwrap();
        assert_eq!(
            admitted.request(),
            &mc_script::ScriptCommand::OpenClientScreen {
                player_id: ScriptPlayerId::new(expected_player),
                screen_id: "example:welcome".to_owned(),
            }
        );
        assert_eq!(
            registry.route_script_client_screen_command(admitted, Some(&manifest)),
            Err(ScriptClientScreenRouteError::PlayerUnavailable)
        );
    }

    let admitted = boundary
        .accept_host_command(boundary.recv_command().await.unwrap())
        .unwrap();
    assert_eq!(
        registry.route_script_client_screen_command(admitted, Some(&manifest)),
        Err(ScriptClientScreenRouteError::ScreenNotOwned)
    );
    let admitted = boundary
        .accept_host_command(boundary.recv_command().await.unwrap())
        .unwrap();
    assert_eq!(
        registry.route_script_client_screen_command(admitted, None),
        Err(ScriptClientScreenRouteError::PluginHasNoEligibleScreenBundle)
    );
}
