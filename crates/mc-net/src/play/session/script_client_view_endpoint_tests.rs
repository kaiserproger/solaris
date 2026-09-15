use std::io::Write as _;

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
            view_kinds: Vec::new(),
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

#[test]
fn a_shipped_client_bundle_declares_the_kinds_its_artifact_index_routes() {
    use std::fs;

    use mc_script::{LuaHostConfig, prepare_lua_plugins};
    use sha2::{Digest, Sha256};

    // A package whose verified artifact declares a settlement screen, exactly
    // as a shipped client bundle does.
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("example-package");
    fs::create_dir_all(&package).unwrap();
    let artifact = package.join("showcase.zip");
    let file = fs::File::create(&artifact).unwrap();
    let mut archive = zip::ZipWriter::new(file);
    archive
        .start_file(
            "solaris-client.json",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    write_index(&mut archive);
    archive.finish().unwrap();
    let bytes = fs::read(&artifact).unwrap();

    fs::write(package.join("main.lua"), "").unwrap();
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let size = bytes.len();
    fs::write(
        package.join("plugin.toml"),
        format!(
            r#"
id = "example"
name = "Example"
version = "1.0.0"
api = "0.6.0"

[client]
schema = 2

[[client.bundles]]
id = "showcase"
version = "1.0.0"
artifact = "showcase.zip"
sha256 = "{sha256}"
size_bytes = {size}
loaders = ["fabric"]
content = ["views"]
permissions = ["present_views"]
"#
        ),
    )
    .unwrap();

    let prepared = prepare_lua_plugins(LuaHostConfig::new(directory.path())).unwrap();
    let manifest = LoaderManifest::from_script_bundles(prepared.client_bundles()).unwrap();
    assert_eq!(
        manifest.declared_view_kinds().collect::<Vec<_>>(),
        vec![("example", ScriptClientViewRequestKind::Settlement)]
    );

    // Production ordering: the bind-time declaration comes from the manifest,
    // not from a test-only hook, so the key-driven request finds its owner.
    let registry = SessionRegistry::new();
    registry.declare_manifest_view_kinds(Some(&manifest));
    assert_eq!(
        registry
            .resolve_view_request_owner(ScriptClientViewRequestKind::Settlement, Some(&manifest)),
        Some("example".to_owned())
    );
    assert_eq!(
        registry.resolve_view_request_owner(ScriptClientViewRequestKind::Army, Some(&manifest)),
        None
    );
}

/// The schema-2 index core reads for routing, written as the artifact's first
/// entry.
fn write_index(archive: &mut zip::ZipWriter<std::fs::File>) {
    archive
        .write_all(
            br#"{"schema":2,"screens":[{"id":"example:overview","kind":"settlement","title":"Overview","widgets":[]}],"world_previews":[],"blocks":[],"items":[],"assets":[],"sounds":[]}"#,
        )
        .unwrap();
}

#[test]
fn the_shipped_settlement_package_routes_its_declared_view_kind() {
    use std::path::Path;

    use mc_script::{LuaHostConfig, prepare_lua_plugins};

    // The package is an independent sibling checkout; without it this test
    // proves nothing and says so instead of inventing a fixture.
    let package_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../solaris-default-plugins")
        .join("solaris-settlements");
    if !package_root.is_dir() {
        return;
    }

    let prepared = prepare_lua_plugins(LuaHostConfig::new(package_root.parent().unwrap())).unwrap();
    let manifest = LoaderManifest::from_script_bundles(prepared.client_bundles()).unwrap();
    assert_eq!(
        manifest.declared_view_kinds().collect::<Vec<_>>(),
        vec![(
            "solaris-settlements",
            ScriptClientViewRequestKind::Settlement
        )]
    );

    let registry = SessionRegistry::new();
    registry.declare_manifest_view_kinds(Some(&manifest));
    assert_eq!(
        registry
            .resolve_view_request_owner(ScriptClientViewRequestKind::Settlement, Some(&manifest)),
        Some("solaris-settlements".to_owned())
    );
}
