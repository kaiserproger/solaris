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
    let status = ProcessCommand::new(env!("CARGO"))
        .args([
            "build",
            "--manifest-path",
            sdk.join("Cargo.toml").to_str().expect("utf-8 path"),
            "--target",
            "wasm32-unknown-unknown",
            "--release",
            "-p",
            "solaris-hello-plugin",
        ])
        .status()
        .expect("the guest build starts");
    assert!(status.success(), "the guest fixture must build");
    let module = sdk.join("target/wasm32-unknown-unknown/release/solaris_hello_plugin.wasm");
    let bytes = std::fs::read(module).expect("the guest module exists");
    wit_component::ComponentEncoder::default()
        .module(&bytes)
        .expect("the guest module carries its component types")
        .validate(true)
        .encode()
        .expect("the guest module encodes as a component")
}
