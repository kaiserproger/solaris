//! The complete first-party component pack starts as one strict deployment.
//!
//! This is deliberately separate from the per-package behavior tests: production
//! admits one package catalog, so duplicate command roots, undeclared grants, or
//! an incompatible real component must fail before any host worker starts.

mod fixture;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mc_plugin_host::{
    DeploymentConfig, DiscoveryMode, HostQueues, PlayerSessions, PluginLimits, start_deployment,
};

struct NoSessions;

impl PlayerSessions for NoSessions {
    fn session_of(&self, _player: &str) -> Option<u64> {
        None
    }
}

struct ComponentSpec {
    id: &'static str,
    crate_name: &'static str,
    wasm_name: &'static str,
    grants: &'static [&'static str],
}

const STANDARD_PACK: &[ComponentSpec] = &[
    ComponentSpec {
        id: "solaris-permissions",
        crate_name: "solaris-permissions-plugin",
        wasm_name: "solaris_permissions_plugin.wasm",
        grants: &["storage"],
    },
    ComponentSpec {
        id: "solaris-essentials",
        crate_name: "solaris-essentials-plugin",
        wasm_name: "solaris_essentials_plugin.wasm",
        grants: &["storage", "player_teleport", "player_queries"],
    },
    ComponentSpec {
        id: "solaris-economy",
        crate_name: "solaris-economy-plugin",
        wasm_name: "solaris_economy_plugin.wasm",
        grants: &["storage"],
    },
    ComponentSpec {
        id: "solaris-towns",
        crate_name: "solaris-towns-plugin",
        wasm_name: "solaris_towns_plugin.wasm",
        grants: &["storage", "zones", "player_queries"],
    },
    ComponentSpec {
        id: "solaris-audit",
        crate_name: "solaris-audit-plugin",
        wasm_name: "solaris_audit_plugin.wasm",
        grants: &["storage"],
    },
    ComponentSpec {
        id: "solaris-settlements",
        crate_name: "solaris-settlements-plugin",
        wasm_name: "solaris_settlements_plugin.wasm",
        grants: &["storage", "resident_work", "structure_operations"],
    },
];

#[test]
fn complete_first_party_component_pack_starts_from_real_guest_artifacts() {
    let root = tempfile::tempdir().expect("deployment root");
    for spec in STANDARD_PACK {
        write_package(root.path(), spec);
    }
    let limits = PluginLimits::default();
    let config = DeploymentConfig {
        root: root.path().to_path_buf(),
        mode: DiscoveryMode::Strict,
        expected: STANDARD_PACK
            .iter()
            .map(|spec| spec.id.to_owned())
            .collect(),
        grants: STANDARD_PACK
            .iter()
            .map(|spec| {
                (
                    spec.id.to_owned(),
                    spec.grants
                        .iter()
                        .map(|grant| (*grant).to_owned())
                        .collect(),
                )
            })
            .collect::<BTreeMap<_, _>>(),
        require_grants: true,
        precommit_hooks: Vec::new(),
    };
    let packages = mc_plugin_host::discover(&config, &limits)
        .expect("the real first-party component deployment is strict-loadable")
        .into_packages();
    let host = start_deployment(
        packages,
        limits,
        HostQueues::default(),
        Arc::new(NoSessions),
    )
    .expect("every first-party guest starts in the same deployment");

    let counters = host.stop();
    assert_eq!(counters.len(), STANDARD_PACK.len());
    assert_eq!(
        counters
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<BTreeSet<_>>(),
        STANDARD_PACK
            .iter()
            .map(|spec| spec.id)
            .collect::<BTreeSet<_>>(),
        "the strict deployment starts every declared package exactly once"
    );
}

fn guest_workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("sdk/rust")
        .canonicalize()
        .expect("guest SDK workspace")
}

fn write_package(root: &Path, spec: &ComponentSpec) {
    let source = guest_workspace_root().join("packages").join(spec.id);
    let package = root.join(spec.id);
    std::fs::create_dir(&package).expect("package directory");
    for file in ["plugin.toml", "config.toml"] {
        std::fs::copy(source.join(file), package.join(file))
            .unwrap_or_else(|error| panic!("copy {file} for {}: {error}", spec.id));
    }
    let bytes = fixture::component_bytes_from_workspace(
        &guest_workspace_root(),
        spec.crate_name,
        spec.wasm_name,
    );
    std::fs::write(package.join("plugin.wasm"), bytes).expect("component artifact");
}
