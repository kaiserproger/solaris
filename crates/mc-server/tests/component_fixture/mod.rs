//! A real component package for the `mc-server` CLI startup tests.
//!
//! The fixture is the SDK's example plugin compiled to `wasm32-unknown-unknown`
//! and encoded into a component with `wit-component`, exactly the way a published
//! package is built. Nothing here hand-writes a module: a package these tests
//! deploy is a component of the contract, so a deployment the server accepts (or
//! refuses) is a real one.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository root, derived from this crate's own manifest directory.
#[must_use]
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repository root")
}

/// The fixture component's bytes.
///
/// The guest build runs once per test process: every test in a binary needs the
/// same artifact, and concurrent guest builds of the same target directory wait
/// on the same package lock.
#[must_use]
pub fn component_bytes() -> &'static [u8] {
    static BYTES: std::sync::LazyLock<Vec<u8>> = std::sync::LazyLock::new(build_component_bytes);
    BYTES.as_slice()
}

fn build_component_bytes() -> Vec<u8> {
    let sdk = repo_root().join("sdk/rust");
    let status = Command::new(env!("CARGO"))
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
    let module =
        std::fs::read(sdk.join("target/wasm32-unknown-unknown/release/solaris_hello_plugin.wasm"))
            .expect("the guest module exists");
    wit_component::ComponentEncoder::default()
        .module(&module)
        .expect("the guest module carries its component types")
        .validate(true)
        .encode()
        .expect("the guest module encodes as a component")
}

/// Deploy package `id` under `root`, with `declarations` appended to its manifest
/// and `config` as its `config.toml`.
///
/// The appended text is the manifest an operator would write - a `[client]`
/// bundle, a `[worldgen]` profile - so a case here exercises the same parsing and
/// artifact verification a deployed package gets.
pub fn deploy_package(root: &Path, id: &str, declarations: &str, config: &str) -> PathBuf {
    let directory = root.join(id);
    std::fs::create_dir_all(&directory).expect("package directory");
    std::fs::write(
        directory.join("plugin.toml"),
        format!(
            "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\napi = \"0.7.0\"\nevents = [\"player.joined\"]\nplayer_commands = [\"hello\"]\n{declarations}"
        ),
    )
    .expect("manifest");
    std::fs::write(directory.join("plugin.wasm"), component_bytes()).expect("artifact");
    std::fs::write(directory.join("config.toml"), config).expect("config");
    directory
}

/// The declaration of one client bundle whose artifact is `content`.
///
/// The cases here ship a real ZIP - its first entry is the index the Loader
/// reads - and the declaration carries the size and SHA-256 of the bytes it was
/// given, so a case that writes different bytes than it declared is the
/// tampered-artifact case rather than a fixture accident.
pub fn client_bundle_declaration(content: &[u8]) -> String {
    format!(
        "\n[client]\nschema = 2\n\n[[client.bundles]]\nid = \"rich-content\"\nversion = \"1\"\nartifact = \"client/rich-content.zip\"\nsha256 = \"{}\"\nsize_bytes = {}\nloaders = [\"fabric\"]\ncontent = [\"items\"]\npermissions = [\"register_items\"]\n",
        sha256_hex(content),
        content.len()
    )
}

/// Write the artifact a bundle declaration points at.
pub fn write_artifact(directory: &Path, artifact: &str, content: &[u8]) {
    let path = directory.join(artifact);
    std::fs::create_dir_all(path.parent().expect("artifact parent")).expect("artifact directory");
    std::fs::write(&path, content).expect("artifact");
}

/// A real artifact: a ZIP whose first entry is the index the Loader reads.
#[must_use]
pub fn artifact_bytes(index: &str) -> Vec<u8> {
    use std::io::Write as _;
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    archive
        .start_file(
            "solaris-client.json",
            zip::write::SimpleFileOptions::default(),
        )
        .expect("start the artifact index");
    archive
        .write_all(index.as_bytes())
        .expect("write the artifact index");
    archive.finish().expect("finish the artifact").into_inner()
}

/// The lowercase hexadecimal SHA-256 of `bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
