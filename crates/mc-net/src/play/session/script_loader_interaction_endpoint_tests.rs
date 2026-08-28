use std::num::NonZeroUsize;

use mc_script::ScriptEventKind;

use crate::loader::LOADER_PROTOCOL_VERSION;
use crate::server::ScriptEventSink;
use crate::{LoaderBundle, LoaderContentKind, LoaderManifest, LoaderPermission, LoaderPlatform};

use super::script_loader_interaction_endpoint::{
    LoaderInteractionRouteError, plugin_has_interaction_bundle, route_client_loader_interaction,
};

fn manifest(owner: &str) -> LoaderManifest {
    LoaderManifest {
        protocol: LOADER_PROTOCOL_VERSION,
        bundles: vec![LoaderBundle {
            owner: owner.to_owned(),
            id: "ui".to_owned(),
            version: "1".to_owned(),
            artifact: "client/ui.zip".to_owned(),
            sha256: "a".repeat(64),
            size_bytes: 1,
            loaders: vec![LoaderPlatform::Fabric],
            content: vec![LoaderContentKind::Interactions],
            permissions: vec![LoaderPermission::SendInteractions],
            cache_key: format!("{owner}:ui/1/{}", "a".repeat(64)),
            source_path: None,
            artifact_bytes: None,
            block_id: None,
            block_name: None,
        }],
    }
}

fn payload(id: &str, body: &str) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(&LOADER_PROTOCOL_VERSION.to_be_bytes());
    payload.extend_from_slice(&(id.len() as u16).to_be_bytes());
    payload.extend_from_slice(id.as_bytes());
    payload.extend_from_slice(&(body.len() as u16).to_be_bytes());
    payload.extend_from_slice(body.as_bytes());
    payload
}

#[test]
fn interaction_policy_requires_exact_owner_content_and_permission() {
    let eligible = manifest("example");
    assert!(plugin_has_interaction_bundle(&eligible, "example"));
    assert!(!plugin_has_interaction_bundle(&eligible, "other"));

    let mut missing_permission = eligible.clone();
    missing_permission.bundles[0].permissions.clear();
    assert!(!plugin_has_interaction_bundle(
        &missing_permission,
        "example"
    ));
}

#[tokio::test]
async fn interaction_routes_only_from_acknowledged_session_to_exact_plugin() {
    let (boundary, mut events) = mc_script::script_boundary_pair(
        NonZeroUsize::new(4).unwrap(),
        NonZeroUsize::new(1).unwrap(),
    );
    let sink = ScriptEventSink::new(boundary);
    let manifest = manifest("example");

    route_client_loader_interaction(
        Some(&sink),
        17,
        true,
        Some(&manifest),
        &payload("example:continue", "accepted"),
    )
    .await
    .unwrap();
    let event = events.recv_event().await.unwrap();
    assert_eq!(event.target_plugin_id(), Some("example"));
    assert!(matches!(
        event.kind(),
        ScriptEventKind::LoaderInteraction {
            player_id,
            interaction_id,
            payload,
        } if player_id.value() == 17
            && interaction_id == "example:continue"
            && payload == "accepted"
    ));

    assert_eq!(
        route_client_loader_interaction(
            Some(&sink),
            17,
            false,
            Some(&manifest),
            &payload("example:continue", "accepted"),
        )
        .await,
        Err(LoaderInteractionRouteError::LoaderNotAcknowledged)
    );
    assert_eq!(
        route_client_loader_interaction(
            Some(&sink),
            17,
            true,
            Some(&manifest),
            &payload("other:continue", "accepted"),
        )
        .await,
        Err(LoaderInteractionRouteError::PluginHasNoEligibleInteractionBundle)
    );
    assert_eq!(
        route_client_loader_interaction(
            Some(&sink),
            17,
            true,
            None,
            &payload("example:continue", "accepted"),
        )
        .await,
        Err(LoaderInteractionRouteError::PluginHasNoEligibleInteractionBundle)
    );
}
