//! What a deployment admits, and what it refuses to run.

use std::collections::BTreeMap;
use std::path::Path;

use mc_plugin_host::{
    DeploymentConfig, DiscoveryError, DiscoveryMode, PluginLimits, discover, load_package,
};

fn write_package(root: &Path, id: &str, manifest_extra: &str) {
    let directory = root.join(id);
    std::fs::create_dir_all(&directory).expect("package directory");
    std::fs::write(
        directory.join("plugin.toml"),
        format!(
            "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\n{manifest_extra}"
        ),
    )
    .expect("manifest");
    std::fs::write(directory.join("plugin.wasm"), b"component-bytes").expect("artifact");
}

fn config(root: &Path, mode: DiscoveryMode) -> DeploymentConfig {
    DeploymentConfig {
        root: root.to_path_buf(),
        mode,
        expected: Vec::new(),
        grants: BTreeMap::new(),
        require_grants: false,
    }
}

#[test]
fn a_strict_deployment_reads_every_package_and_sorts_them() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path(), "zulu", "");
    write_package(
        root.path(),
        "alpha",
        "capabilities = [\"player_inventory\"]\n",
    );
    let mut strict = config(root.path(), DiscoveryMode::Strict);
    strict.expected = vec!["alpha".to_owned(), "zulu".to_owned()];
    let deployment = discover(&strict, &PluginLimits::default()).expect("deployment loads");
    let ids = deployment
        .packages()
        .iter()
        .map(|package| package.manifest().plugin_id().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["alpha".to_owned(), "zulu".to_owned()]);
    assert!(deployment.skipped().is_empty());
}

#[test]
fn strict_mode_refuses_a_stray_entry_and_permissive_mode_skips_a_broken_package() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path(), "alpha", "");
    std::fs::write(root.path().join("stray.txt"), b"not a package").expect("stray file");
    let error = discover(
        &config(root.path(), DiscoveryMode::Strict),
        &PluginLimits::default(),
    )
    .expect_err("strict discovery permits only plugin directories");
    assert!(
        matches!(error, DiscoveryError::NotAPackage { .. }),
        "{error}"
    );

    std::fs::remove_file(root.path().join("stray.txt")).expect("remove stray");
    let broken = root.path().join("broken");
    std::fs::create_dir_all(&broken).expect("broken package");
    std::fs::write(broken.join("plugin.toml"), "id = \"broken\"\n").expect("broken manifest");
    let permissive = discover(
        &config(root.path(), DiscoveryMode::Permissive),
        &PluginLimits::default(),
    )
    .expect("permissive discovery skips the broken package");
    assert_eq!(permissive.packages().len(), 1);
    assert_eq!(
        permissive.skipped().len(),
        1,
        "a skipped package is reported, not hidden"
    );
    assert!(permissive.skipped()[0].path.ends_with("broken"));
}

#[test]
fn two_packages_with_one_id_are_refused_in_both_modes() {
    for mode in [DiscoveryMode::Strict, DiscoveryMode::Permissive] {
        let root = tempfile::tempdir().expect("deployment root");
        write_package(root.path(), "alpha", "");
        let second = root.path().join("alpha-copy");
        std::fs::create_dir_all(&second).expect("second package");
        std::fs::write(
            second.join("plugin.toml"),
            "id = \"alpha\"\nname = \"alpha\"\nversion = \"0.2.0\"\napi = \"0.7.0\"\n",
        )
        .expect("second manifest");
        std::fs::write(second.join("plugin.wasm"), b"component-bytes").expect("artifact");
        let error = discover(&config(root.path(), mode), &PluginLimits::default())
            .expect_err("a duplicate id must fail");
        assert!(
            matches!(error, DiscoveryError::DuplicateId { .. }),
            "{error}"
        );
    }
}

#[test]
fn expected_ids_are_required_and_unexpected_ids_are_refused() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path(), "alpha", "");
    let mut settings = config(root.path(), DiscoveryMode::Strict);
    settings.expected = vec!["alpha".to_owned(), "beta".to_owned()];
    let error = discover(&settings, &PluginLimits::default()).expect_err("beta is missing");
    assert!(
        matches!(error, DiscoveryError::ExpectedMissing { ref id } if id == "beta"),
        "{error}"
    );

    let mut settings = config(root.path(), DiscoveryMode::Strict);
    write_package(root.path(), "gamma", "");
    settings.expected = vec!["alpha".to_owned()];
    let error = discover(&settings, &PluginLimits::default()).expect_err("gamma is unexpected");
    assert!(
        matches!(error, DiscoveryError::Unexpected { ref id } if id == "gamma"),
        "{error}"
    );
}

#[test]
fn strict_mode_requires_the_exact_id_set_the_operator_declared() {
    // The production contract, unchanged from the Luau deployment: strict means
    // the discovered set equals the declared one, so nothing runs unless it was
    // declared, and an empty declaration admits no package at all.
    let root = tempfile::tempdir().expect("deployment root");
    write_package(root.path(), "alpha", "");
    let error = discover(
        &config(root.path(), DiscoveryMode::Strict),
        &PluginLimits::default(),
    )
    .expect_err("an undeclared package must not run");
    assert!(
        matches!(error, DiscoveryError::Unexpected { ref id } if id == "alpha"),
        "{error}"
    );

    let mut empty_expected = config(root.path(), DiscoveryMode::Strict);
    empty_expected.expected = vec![String::new()];
    let error = discover(&empty_expected, &PluginLimits::default())
        .expect_err("an empty expected id is malformed");
    assert!(
        matches!(error, DiscoveryError::ExpectedSet { .. }),
        "{error}"
    );
}

#[test]
fn a_requested_capability_without_a_grant_fails_the_package() {
    let root = tempfile::tempdir().expect("deployment root");
    write_package(
        root.path(),
        "alpha",
        "capabilities = [\"player_inventory\"]\n",
    );
    let mut settings = config(root.path(), DiscoveryMode::Strict);
    settings.expected = vec!["alpha".to_owned()];
    settings.require_grants = true;
    let error = discover(&settings, &PluginLimits::default())
        .expect_err("an ungranted capability must fail the package, not be truncated");
    assert!(
        matches!(error, DiscoveryError::Ungranted { ref capability, .. } if capability == "player_inventory"),
        "{error}"
    );

    let mut granted = settings.clone();
    granted
        .grants
        .insert("alpha".to_owned(), vec!["player_inventory".to_owned()]);
    let deployment =
        discover(&granted, &PluginLimits::default()).expect("the granted package runs");
    assert_eq!(deployment.packages().len(), 1);
    assert_eq!(
        deployment.packages()[0].requested_capabilities(),
        ["player_inventory".to_owned()]
    );
}

#[test]
fn a_package_that_asks_for_another_contract_version_is_refused_by_discovery() {
    let root = tempfile::tempdir().expect("deployment root");
    let directory = root.path().join("legacy");
    std::fs::create_dir_all(&directory).expect("package directory");
    std::fs::write(
        directory.join("plugin.toml"),
        "id = \"legacy\"\nname = \"legacy\"\nversion = \"0.1.0\"\napi = \"0.6.0\"\n",
    )
    .expect("manifest");
    std::fs::write(directory.join("plugin.wasm"), b"component-bytes").expect("artifact");
    let error = load_package(&directory, &PluginLimits::default())
        .expect_err("the Luau contract version is not the component one");
    assert!(format!("{error}").contains("invalid manifest"), "{error}");
}
