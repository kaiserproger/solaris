//! Encode one core guest module into a component, the way a published package is built.
//!
//! `tools/build-loader-live-gate-fixture.sh --prepare` runs this example between
//! the SDK guest build and the deployment root: the guest is compiled to
//! `wasm32-unknown-unknown`, and a package's `plugin.wasm` entry has to be the
//! component the host compiles, not the core module the toolchain leaves behind.
//! Encoding with the same encoder the host's own tests use is the point - a
//! hand-written or hand-wrapped module is not a contract component, so a fixture
//! built from one would prove nothing about a real plugin.
//!
//! Usage: `plugin-component <module.wasm> <component.wasm>`

use std::path::PathBuf;

fn main() {
    let mut arguments = std::env::args_os().skip(1);
    let module = arguments
        .next()
        .map(PathBuf::from)
        .expect("usage: plugin-component <module.wasm> <component.wasm>");
    let output = arguments
        .next()
        .map(PathBuf::from)
        .expect("usage: plugin-component <module.wasm> <component.wasm>");
    assert!(
        arguments.next().is_none(),
        "usage: plugin-component <module.wasm> <component.wasm>"
    );

    let bytes = std::fs::read(&module)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", module.display()));
    let component = wit_component::ComponentEncoder::default()
        .module(&bytes)
        .expect("the guest module carries its component types")
        .validate(true)
        .encode()
        .expect("the guest module encodes as a component");
    std::fs::write(&output, component)
        .unwrap_or_else(|error| panic!("cannot write {}: {error}", output.display()));
}
