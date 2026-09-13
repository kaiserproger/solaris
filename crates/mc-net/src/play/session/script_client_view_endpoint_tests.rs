use mc_script::ScriptClientViewRequestKind;

use crate::{
    LOADER_PROTOCOL_VERSION, LoaderBundle, LoaderContentKind, LoaderManifest, LoaderPermission,
    LoaderPlatform,
};

use super::SessionRegistry;

fn manifest(content: Vec<LoaderContentKind>, permissions: Vec<LoaderPermission>) -> LoaderManifest {
    LoaderManifest {
        protocol: LOADER_PROTOCOL_VERSION,
        bundles: vec![LoaderBundle {
            owner: "example".to_owned(),
            id: "showcase".to_owned(),
            version: "1".to_owned(),
            artifact: "client/showcase.zip".to_owned(),
            sha256: "a".repeat(64),
            size_bytes: 1,
            loaders: vec![LoaderPlatform::Fabric],
            content,
            permissions,
            cache_key: format!("example:showcase/1/{}", "a".repeat(64)),
            source_path: None,
            artifact_bytes: None,
            block_id: None,
            block_name: None,
        }],
    }
}

fn views_manifest() -> LoaderManifest {
    manifest(
        vec![LoaderContentKind::Views, LoaderContentKind::ViewActions],
        vec![
            LoaderPermission::PresentViews,
            LoaderPermission::SendViewActions,
        ],
    )
}

#[test]
fn declared_view_kind_reaches_its_owner_and_undeclared_kinds_open_nothing() {
    let registry = SessionRegistry::new();
    registry.declare_loader_view_kinds("example", [ScriptClientViewRequestKind::Settlement]);
    let manifest = views_manifest();

    assert_eq!(
        registry
            .resolve_view_request_owner(ScriptClientViewRequestKind::Settlement, Some(&manifest)),
        Some("example".to_owned())
    );
    // The plugin does not declare an army view, so nothing opens.
    assert_eq!(
        registry.resolve_view_request_owner(ScriptClientViewRequestKind::Army, Some(&manifest)),
        None
    );
    // With no live manifest there is no owner to notify.
    assert_eq!(
        registry.resolve_view_request_owner(ScriptClientViewRequestKind::Settlement, None),
        None
    );
}

#[test]
fn view_request_owner_requires_the_present_views_permission_pair() {
    let registry = SessionRegistry::new();
    registry.declare_loader_view_kinds("example", [ScriptClientViewRequestKind::Settlement]);

    let missing_permission = manifest(vec![LoaderContentKind::Views], vec![]);
    assert_eq!(
        registry.resolve_view_request_owner(
            ScriptClientViewRequestKind::Settlement,
            Some(&missing_permission)
        ),
        None
    );
    let missing_content = manifest(vec![], vec![LoaderPermission::PresentViews]);
    assert_eq!(
        registry.resolve_view_request_owner(
            ScriptClientViewRequestKind::Settlement,
            Some(&missing_content)
        ),
        None
    );
}

#[test]
fn ambiguous_view_kind_owners_open_nothing() {
    let registry = SessionRegistry::new();
    registry.declare_loader_view_kinds("example", [ScriptClientViewRequestKind::Settlement]);
    registry.declare_loader_view_kinds("other", [ScriptClientViewRequestKind::Settlement]);
    assert_eq!(
        registry.resolve_view_request_owner(
            ScriptClientViewRequestKind::Settlement,
            Some(&views_manifest())
        ),
        None
    );
}
