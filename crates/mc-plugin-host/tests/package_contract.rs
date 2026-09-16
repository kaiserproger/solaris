//! The package contract: what a component package directory may say, and what is
//! refused before any guest code exists.

use mc_plugin_host::{PackageError, PluginLimits, load_package};

/// One package directory with `plugin.toml` and an artifact of `artifact` bytes.
fn package(manifest: &str, artifact: &[u8]) -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("temp package root");
    std::fs::write(root.path().join("plugin.toml"), manifest).expect("manifest");
    if !artifact.is_empty() {
        std::fs::write(root.path().join("plugin.wasm"), artifact).expect("artifact");
    }
    root
}

fn manifest(api: &str, extra: &str) -> String {
    format!("id = \"hello\"\nname = \"Hello\"\nversion = \"0.1.0\"\napi = \"{api}\"\n{extra}")
}

#[test]
fn a_component_package_loads_with_its_manifest_contract() {
    let root = package(
        &manifest(
            "0.7.0",
            "events = [\"player.joined\"]\ncapabilities = [\"player_inventory\"]\nplayer_commands = [\"hello\"]\n",
        ),
        b"component-bytes",
    );
    let package = load_package(root.path(), &PluginLimits::default()).expect("package loads");
    assert_eq!(package.manifest().plugin_id(), "hello");
    assert_eq!(package.artifact(), b"component-bytes");
    assert_eq!(package.root(), root.path());
    assert!(
        package
            .manifest()
            .player_command_roots()
            .contains(&"hello".to_owned()),
        "the declared command root reaches the manifest contract"
    );
}

#[test]
fn the_component_contract_does_not_admit_a_luau_api_version() {
    // The two runtimes have independent versions: a package asking for the Luau
    // contract is refused here, and asking for the component contract is refused
    // there, without loosening either check.
    let root = package(&manifest("0.6.0", ""), b"component-bytes");
    let error = load_package(root.path(), &PluginLimits::default())
        .expect_err("the Luau contract is not the component contract");
    assert!(
        matches!(error, PackageError::Manifest { .. }),
        "the refusal names the manifest: {error}"
    );
}

#[test]
fn an_unknown_capability_is_refused_rather_than_ignored() {
    let root = package(
        &manifest("0.7.0", "capabilities = [\"chat.send\"]\n"),
        b"component-bytes",
    );
    let error = load_package(root.path(), &PluginLimits::default())
        .expect_err("an unknown capability must fail the package");
    assert!(
        format!("{error}").contains("capability"),
        "the refusal names the capability: {error}"
    );
}

#[test]
fn an_entry_that_leaves_the_package_is_refused() {
    for entry in ["../outside.wasm", "/etc/passwd", ""] {
        let root = package(
            &manifest("0.7.0", &format!("entry = \"{entry}\"\n")),
            b"component-bytes",
        );
        let error = load_package(root.path(), &PluginLimits::default())
            .expect_err("an entry outside the package must be refused");
        assert!(
            matches!(error, PackageError::Escape { .. } | PackageError::Io { .. }),
            "entry {entry:?} was refused as {error}"
        );
    }
}

#[test]
fn an_artifact_past_the_bound_is_refused_before_compilation() {
    let root = package(&manifest("0.7.0", ""), &vec![0_u8; 4096]);
    let limits = PluginLimits {
        artifact_bytes: 1024,
        ..PluginLimits::default()
    };
    let error = load_package(root.path(), &limits).expect_err("the artifact bound applies");
    assert!(
        matches!(error, PackageError::ArtifactTooLarge { .. }),
        "the refusal is about size, not compilation: {error}"
    );
}
