//! The one guest fixture every host test binary builds.
//!
//! The fixture is a real Rust plugin (`sdk/rust/examples/hello`) compiled to
//! `wasm32-unknown-unknown` and encoded into a component with `wit-component`,
//! exactly the way a published package is built. Nothing in these tests
//! hand-writes a module: if the WIT, the SDK and the host disagree by one type,
//! the fixture either does not build or does not instantiate.
//!
//! This module is included by several test binaries, so it holds only what they
//! all need: the fixture's bytes and the repository root. What a test's host
//! services do (and how the fixture is driven) stays in the test that owns it.
#![allow(
    dead_code,
    reason = "each integration-test binary uses a different subset of the shared fixture helpers"
)]

use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

/// The repository root, derived from this crate's own manifest directory.
#[must_use]
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate lives under crates/")
        .to_path_buf()
}

/// The fixture plugin's component bytes.
///
/// The guest build runs once per test process: every test in a binary needs the
/// same artifact, and concurrent guest builds of the same target directory wait
/// on the same package lock.
#[must_use]
pub fn component_bytes() -> Vec<u8> {
    static BYTES: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(build_component_bytes);
    BYTES.clone()
}

fn build_component_bytes() -> Vec<u8> {
    let sdk = repo_root().join("sdk/rust");
    component_bytes_from_workspace(&sdk, "solaris-hello-plugin", "solaris_hello_plugin.wasm")
}

/// Build an SDK-workspace test guest and encode its real core module as a
/// component.
///
/// Component integration tests use this instead of hand-written wasm: the test
/// guest must compile against the same WIT contract as the host.
#[must_use]
pub fn component_bytes_from_workspace(
    workspace: &Path,
    package: &str,
    module_name: &str,
) -> Vec<u8> {
    let manifest = workspace.join("Cargo.toml");
    let status = ProcessCommand::new(env!("CARGO"))
        .args([
            "build",
            "--manifest-path",
            manifest.to_str().expect("utf-8 workspace path"),
            "--target",
            "wasm32-unknown-unknown",
            "--release",
            "-p",
            package,
        ])
        .status()
        .expect("guest build starts");
    assert!(status.success(), "{package} guest build succeeds");
    let module = workspace
        .join("target/wasm32-unknown-unknown/release")
        .join(module_name);
    let bytes = std::fs::read(module).expect("guest module exists");
    wit_component::ComponentEncoder::default()
        .module(&bytes)
        .expect("guest module carries component types")
        .validate(true)
        .encode()
        .expect("guest module encodes as a component")
}
