use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use mc_plugin_host::{DeploymentConfig, DiscoveryMode, PluginLimits, discover};

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

fn strict_package(root: &Path, id: &str) -> mc_plugin_host::LoadedPackage {
    let package = root.join(id);
    std::fs::create_dir_all(&package).expect("package directory");
    std::fs::write(package.join("plugin.wasm"), hello_component_bytes())
        .expect("component artifact");
    std::fs::write(package.join("config.toml"), "").expect("component config");
    discover(
        &DeploymentConfig {
            root: root.to_path_buf(),
            mode: DiscoveryMode::Strict,
            expected: vec![id.to_owned()],
            grants: BTreeMap::new(),
            require_grants: false,
            precommit_hooks: Vec::new(),
        },
        &PluginLimits::default(),
    )
    .expect("strict component package discovers")
    .into_packages()
    .pop()
    .expect("one package")
}

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

    use sha2::{Digest, Sha256};

    // A strict component package whose verified artifact declares a settlement
    // screen, exactly as a shipped client bundle does.
    let directory = tempfile::tempdir().unwrap();
    let package = directory.path().join("example");
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
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let size = bytes.len();
    fs::write(
        package.join("plugin.toml"),
        format!(
            r#"id = "example"
name = "Example"
version = "1.0.0"
api = "0.7.0"

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
    let package = strict_package(directory.path(), "example");
    let manifest = LoaderManifest::from_script_bundles(package.client_bundles()).unwrap();
    assert_eq!(
        manifest.declared_view_kinds().collect::<Vec<_>>(),
        vec![("example", ScriptClientViewRequestKind::Settlement)]
    );

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
