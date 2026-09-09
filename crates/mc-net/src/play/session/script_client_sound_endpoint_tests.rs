use std::collections::{BTreeMap, HashSet};

use mc_script::{LuaHostConfig, ScriptEvent, start_lua_host};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::script_client_sound_endpoint::ScriptClientSoundRouteError;
use super::{SessionRegistration, SessionRegistry};
use crate::login::LoggedInProfile;
use crate::play::PlayerPose;
use crate::{
    LOADER_PROTOCOL_VERSION, LoaderBundle, LoaderClientAck, LoaderContentKind, LoaderManifest,
    LoaderPermission, LoaderPlatform,
};

#[tokio::test]
async fn sound_commands_cannot_cross_owner_permission_or_live_session_boundaries() {
    let manifest = LoaderManifest {
        protocol: LOADER_PROTOCOL_VERSION,
        bundles: vec![LoaderBundle {
            owner: "example".to_owned(),
            id: "sound".to_owned(),
            version: "1".to_owned(),
            artifact: "client/sound.zip".to_owned(),
            sha256: "a".repeat(64),
            size_bytes: 1,
            loaders: vec![LoaderPlatform::Fabric],
            content: vec![LoaderContentKind::Sounds],
            permissions: vec![LoaderPermission::PlaySounds],
            cache_key: format!("example:sound/1/{}", "a".repeat(64)),
            source_path: None,
            artifact_bytes: None,
            block_id: None,
            block_name: None,
        }],
    };
    let session = manifest
        .bind_ack(&LoaderClientAck {
            protocol: LOADER_PROTOCOL_VERSION,
            platform: LoaderPlatform::Fabric,
            loader_version: "test".to_owned(),
            accepted_permissions: vec![LoaderPermission::PlaySounds],
            cached_bundles: vec![manifest.bundles[0].cache_key.clone()],
            carrier_block_state_ids: BTreeMap::new(),
        })
        .unwrap();
    let registry = SessionRegistry::new();
    let eligible = LoggedInProfile {
        uuid: Uuid::from_u128(1),
        name: "Eligible".to_owned(),
    };
    let vanilla = LoggedInProfile {
        uuid: Uuid::from_u128(2),
        name: "Vanilla".to_owned(),
    };
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
    let (tx, mut rx) = mpsc::channel(4);
    let (eligible_id, _) = registry
        .try_register(registration(&eligible, tx, Some(session)))
        .unwrap();
    let (tx, mut vanilla_rx) = mpsc::channel(4);
    let (vanilla_id, _) = registry
        .try_register(registration(&vanilla, tx, None))
        .unwrap();
    let plugins = tempfile::tempdir().unwrap();
    let plugin = plugins.path().join("example");
    std::fs::create_dir(&plugin).unwrap();
    std::fs::write(plugin.join("plugin.toml"),
        "id = \"example\"\nname = \"Example\"\nversion = \"0.1.0\"\napi = \"0.6.0\"\nevents = [\"server.started\"]\n").unwrap();
    std::fs::write(
        plugin.join("main.lua"),
        format!(
            r#"
function on_server_started(_event)
    solaris.play_client_sound({eligible_id}, "example:tone", {{}})
    solaris.stop_client_sound({eligible_id}, "other:tone")
    solaris.stop_client_sound({vanilla_id}, "example:tone")
    solaris.stop_client_sound(999999, "example:tone")
    solaris.stop_client_sound({eligible_id}, "example:tone")
    solaris.stop_client_sound({eligible_id}, "example:tone")
    solaris.stop_client_sound({eligible_id}, "example:tone")
    solaris.stop_client_sound({eligible_id}, "example:tone")
end
"#
        ),
    )
    .unwrap();
    let (boundary, _host) = start_lua_host(LuaHostConfig::new(plugins.path())).unwrap();
    boundary
        .try_enqueue_event(ScriptEvent::server_started())
        .unwrap();
    let admitted = boundary
        .accept_host_command(boundary.recv_command().await.unwrap())
        .unwrap();
    registry
        .route_script_client_sound_command(admitted, Some(&manifest))
        .unwrap();
    rx.recv().await.unwrap();
    for error in [
        ScriptClientSoundRouteError::SoundNotOwned,
        ScriptClientSoundRouteError::PlayerUnavailable,
        ScriptClientSoundRouteError::PlayerUnavailable,
    ] {
        let admitted = boundary
            .accept_host_command(boundary.recv_command().await.unwrap())
            .unwrap();
        assert_eq!(
            registry.route_script_client_sound_command(admitted, Some(&manifest)),
            Err(error)
        );
    }
    for missing in 0..3 {
        let mut ineligible = manifest.clone();
        match missing {
            0 => ineligible.bundles[0].owner = "other".to_owned(),
            1 => ineligible.bundles[0].content.clear(),
            _ => ineligible.bundles[0].permissions.clear(),
        }
        let admitted = boundary
            .accept_host_command(boundary.recv_command().await.unwrap())
            .unwrap();
        assert_eq!(
            registry.route_script_client_sound_command(admitted, Some(&ineligible)),
            Err(ScriptClientSoundRouteError::PluginHasNoEligibleSoundBundle)
        );
    }
    assert!(rx.try_recv().is_err());
    assert!(vanilla_rx.try_recv().is_err());
    drop(rx);
    let admitted = boundary
        .accept_host_command(boundary.recv_command().await.unwrap())
        .unwrap();
    assert_eq!(
        registry.route_script_client_sound_command(admitted, Some(&manifest)),
        Err(ScriptClientSoundRouteError::PlayerUnavailable)
    );
}
