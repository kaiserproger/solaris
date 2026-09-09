use std::collections::{BTreeMap, HashSet};

use bytes::Buf;
use mc_script::{LuaHostConfig, ScriptEvent, start_lua_host};
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::loader::loader_ui_channel;
use crate::{
    LOADER_PROTOCOL_VERSION, LoaderBundle, LoaderClientAck, LoaderContentKind, LoaderManifest,
    LoaderPermission, LoaderPlatform,
};

use super::outbound::OutboundCommand;
use super::script_client_ui_endpoint::{
    ScriptClientUiRouteError, plugin_has_ui_bundle, ui_is_owned,
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
            artifact_bytes: None,
            block_id: None,
            block_name: None,
        }],
    }
}

#[test]
fn ui_policy_requires_matching_owner_content_and_permission() {
    let eligible = manifest(
        vec![LoaderContentKind::Ui],
        vec![LoaderPermission::PresentUi],
    );
    assert!(plugin_has_ui_bundle(&eligible, "example"));
    assert!(!plugin_has_ui_bundle(&eligible, "other"));
    assert!(!plugin_has_ui_bundle(
        &manifest(
            vec![LoaderContentKind::Assets],
            vec![LoaderPermission::LoadAssets],
        ),
        "example",
    ));
}

#[test]
fn admitted_plugin_can_only_name_its_own_ui_namespace() {
    assert!(ui_is_owned("example", "example:welcome"));
    assert!(!ui_is_owned("example", "other:welcome"));
    assert!(!ui_is_owned("example", "example:"));
}

fn profile(id: u128, name: &str) -> LoggedInProfile {
    LoggedInProfile {
        uuid: Uuid::from_u128(id),
        name: name.to_owned(),
    }
}

fn ui_text<'a>(payload: &mut &'a [u8]) -> Option<&'a str> {
    let len = payload.get_u16();
    if len == u16::MAX {
        return None;
    }
    let (text, remaining) = (*payload).split_at(usize::from(len));
    *payload = remaining;
    Some(std::str::from_utf8(text).expect("UI text uses UTF-8"))
}

#[tokio::test]
async fn admitted_ui_modes_require_exact_loader_session_and_publish_bounded_content() {
    let registry = SessionRegistry::new();
    let eligible_profile = profile(1, "Eligible");
    let vanilla_profile = profile(2, "Vanilla");
    let closed_profile = profile(3, "Closed");
    let (eligible_tx, mut eligible_rx) = mpsc::channel(4);
    let (vanilla_tx, _vanilla_rx) = mpsc::channel(4);
    let (closed_tx, closed_rx) = mpsc::channel(4);
    drop(closed_rx);

    let mut loader_manifest = manifest(
        vec![LoaderContentKind::Ui, LoaderContentKind::Blocks],
        vec![
            LoaderPermission::PresentUi,
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
    solaris.present_client_ui({eligible_id}, "example:welcome", {{ mode = "screen" }})
    solaris.present_client_ui({eligible_id}, "example:welcome", {{ mode = "hud", title = "Status", body = "Health: 20" }})
    solaris.present_client_ui({eligible_id}, "example:welcome", {{ mode = "hidden" }})
    solaris.present_client_ui({vanilla_id}, "example:welcome", {{ mode = "hud" }})
    solaris.present_client_ui({closed_id}, "example:welcome", {{ mode = "hud" }})
    solaris.present_client_ui(999999, "example:welcome", {{ mode = "hud" }})
    solaris.present_client_ui({eligible_id}, "other:welcome", {{ mode = "hud" }})
    solaris.present_client_ui({eligible_id}, "example:welcome", {{ mode = "hud" }})
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
        vec![LoaderContentKind::Ui],
        vec![LoaderPermission::PresentUi],
    );

    for (mode, title, body) in [
        (0, None, None),
        (1, Some("Status"), Some("Health: 20")),
        (2, None, None),
    ] {
        let admitted = boundary
            .accept_host_command(boundary.recv_command().await.unwrap())
            .unwrap();
        registry
            .route_script_client_ui_command(admitted, Some(&manifest))
            .unwrap();
        let OutboundCommand::CustomPayload { channel, payload } = eligible_rx.recv().await.unwrap()
        else {
            panic!("expected Loader UI payload");
        };
        assert_eq!(channel, *loader_ui_channel());
        let mut payload = payload.as_slice();
        assert_eq!(payload.get_u16(), LOADER_PROTOCOL_VERSION);
        assert_eq!(payload.get_u8(), mode);
        assert_eq!(ui_text(&mut payload), Some("example:welcome"));
        assert_eq!(ui_text(&mut payload), title);
        assert_eq!(ui_text(&mut payload), body);
        assert!(payload.is_empty());
    }

    for _ in [vanilla_id, closed_id, 999999] {
        let admitted = boundary
            .accept_host_command(boundary.recv_command().await.unwrap())
            .unwrap();
        assert_eq!(
            registry.route_script_client_ui_command(admitted, Some(&manifest)),
            Err(ScriptClientUiRouteError::PlayerUnavailable)
        );
    }

    let admitted = boundary
        .accept_host_command(boundary.recv_command().await.unwrap())
        .unwrap();
    assert_eq!(
        registry.route_script_client_ui_command(admitted, Some(&manifest)),
        Err(ScriptClientUiRouteError::UiNotOwned)
    );
    let admitted = boundary
        .accept_host_command(boundary.recv_command().await.unwrap())
        .unwrap();
    assert_eq!(
        registry.route_script_client_ui_command(admitted, None),
        Err(ScriptClientUiRouteError::PluginHasNoEligibleUiBundle)
    );
}
