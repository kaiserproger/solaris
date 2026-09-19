//! P0 acceptance: a component that matches the version string but not the
//! contract is refused before the game host admits it.
//!
//! `component_roundtrip.rs` covers one half of the plan's row - bytes that are
//! not a component at all - and refuses them before compilation. This binary
//! covers the other half: a real component, encoded from a WIT document whose
//! package version is exactly the one the host accepts, whose *shape* the host's
//! linker cannot satisfy. The version string is not a pass: the package's own
//! manifest gate admits it, and the contract's types refuse it.
//!
//! Nothing here hand-writes a module. Each component is `wit-component`'s dummy
//! module for a WIT document - a core module whose every exported body is
//! `unreachable` - encoded into a component the same way a published package is
//! encoded. The two cases are a matched pair: the same builder produces a
//! component the host admits and one it must refuse, so neither answer can come
//! from the bytes being unreadable, unbuildable or empty.

use std::path::{Path, PathBuf};

use mc_plugin_host::{
    HostError, HostServices, LoadedPackage, LogLevel, PluginInstance, PluginLimits, PluginStartup,
    engine, linker, load_package, package::compile_package,
};
use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve, WorldId};

mod fixture;

/// The services a case's instance is given.
///
/// Both cases are decided before a callback can log: the foreign world never
/// instantiates, and the dummy guest's only body is `unreachable`, so the
/// contract's imports are never reached and there is nothing to capture.
#[derive(Default)]
struct Services;

impl HostServices for Services {
    fn log(&mut self, _level: LogLevel, _message: &str) {}

    fn plugin_id(&self) -> &str {
        "contract-refusal"
    }
}

/// The contract's WIT, the document every real package is built against.
fn real_wit() -> PathBuf {
    fixture::repo_root().join("crates/mc-script/wit")
}

/// Parse the WIT in `dir`: its `plugin` world, and the version its package
/// declares.
///
/// The package version is what a package's manifest carries as `api`, so a case
/// can build a manifest that the host's own version gate accepts without
/// restating the version here.
fn plugin_world(dir: &Path) -> (Resolve, WorldId, String) {
    let mut resolve = Resolve::new();
    let (package, _) = resolve.push_path(dir).expect("the WIT parses");
    // `PackageName` prints as `namespace:name@version`, which is the text a
    // package's `api` field carries.
    let name = resolve.packages[package].name.to_string();
    let version = name
        .rsplit_once('@')
        .expect("the WIT package declares a version")
        .1
        .to_owned();
    let world = resolve
        .select_world(&[package], Some("plugin"))
        .expect("the WIT package declares the `plugin` world");
    (resolve, world, version)
}

/// Encode a component implementing `world` whose every exported body is
/// `unreachable`: the shape is the WIT's, the behaviour is nothing.
fn dummy_component(resolve: &Resolve, world: WorldId) -> Vec<u8> {
    let mut module = dummy_module(resolve, world, ManglingAndAbi::Standard32);
    // The dummy module is a plain core module and carries no component types of
    // its own, so the world it implements has to be embedded before the encoder
    // can see which component it is being asked to become.
    embed_component_metadata(&mut module, resolve, world, StringEncoding::UTF8)
        .expect("the dummy module takes the world's component types");
    ComponentEncoder::default()
        .module(&module)
        .expect("the dummy module carries its component types")
        .validate(true)
        .encode()
        .expect("the dummy module encodes as a component")
}

/// The real WIT in a fresh directory, with `edit` applied to `plugin.wit`'s text.
///
/// The copy is a whole WIT package, not one file: the other interfaces still have
/// to resolve, so an edit that does not parse fails here instead of quietly
/// turning the case into "unreadable bytes".
fn wit_with(edit: impl Fn(&str) -> String) -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("temporary WIT root");
    let mut entries: Vec<PathBuf> = std::fs::read_dir(real_wit())
        .expect("the contract's WIT directory is readable")
        .map(|entry| entry.expect("a WIT file").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "wit"))
        .collect();
    entries.sort();
    for path in entries {
        let name = path.file_name().expect("a WIT file name").to_owned();
        let text = std::fs::read_to_string(&path).expect("a WIT file reads");
        let text = if name == "plugin.wit" {
            edit(&text)
        } else {
            text
        };
        std::fs::write(directory.path().join(name), text).expect("a WIT file writes");
    }
    directory
}

/// One package directory: a manifest declaring contract version `api`, and
/// `bytes` as its artifact.
fn package(api: &str, bytes: &[u8], limits: &PluginLimits) -> (tempfile::TempDir, LoadedPackage) {
    let root = tempfile::tempdir().expect("temporary package root");
    std::fs::write(
        root.path().join("plugin.toml"),
        format!("id = \"refusal\"\nname = \"Refusal\"\nversion = \"0.1.0\"\napi = \"{api}\"\n"),
    )
    .expect("manifest");
    std::fs::write(root.path().join("plugin.wasm"), bytes).expect("artifact");
    let loaded = load_package(root.path(), limits).expect("the package loads");
    (root, loaded)
}

#[test]
fn a_component_that_matches_the_version_but_not_the_contract_is_refused_at_instantiation() {
    // One semantic change to the real WIT: the world imports an interface the
    // host's linker has never heard of. Every exported interface keeps its name
    // and its types, and the package version - the string the host's own gate
    // admits - is untouched, so nothing but the contract's shape can refuse this
    // package.
    let wit = wit_with(|plugin_wit| {
        format!(
            "{}\ninterface telemetry {{\n    report: func(kind: string, payload: string);\n}}\n",
            plugin_wit.replace(
                "    import host;",
                "    import host;\n    import telemetry;"
            )
        )
    });
    let (resolve, world, api) = plugin_world(wit.path());
    let bytes = dummy_component(&resolve, world);

    let limits = PluginLimits::default();
    // The manifest carries the version the edited WIT declares, and it loads:
    // the version gate admits this package before its shape is ever looked at.
    let (_root, package) = package(&api, &bytes, &limits);
    let engine = engine(&limits).expect("engine");
    let compiled = compile_package(&engine, &package, &limits).expect("the bytes compile");
    let linker = linker::<Services>(&engine).expect("linker");

    let error = PluginInstance::instantiate(&linker, compiled.component(), Services, limits)
        .err()
        .expect("a world the linker cannot satisfy is refused before any guest code runs");

    // A package whose shape does not match is a broken package, not a guest that
    // misbehaved: the operator fixes the first, and the host retires one instance
    // for the second. Reporting this as a trap is the behaviour that must not
    // come back.
    assert!(
        !matches!(error, HostError::Trap(_)),
        "a link mismatch is not a guest fault: {error}"
    );
    assert!(
        matches!(error, HostError::Instantiate(_)),
        "the refusal happens at instantiation, so it is an `Instantiate`: {error}"
    );
    // The refusal names what the host could not link, which is the import the
    // edited world added - not the version, which matched.
    assert!(
        format!("{error}").contains("telemetry"),
        "the refusal names the unsatisfied import: {error}"
    );
}

#[test]
fn a_component_of_the_contract_is_admitted_and_its_fault_is_reported_as_a_guest_trap() {
    // The unmodified WIT: the same document a published package is built against,
    // so the same builder that produced the refused component above produces this
    // one, and the only difference between the two is the world.
    let wit = real_wit();
    let (resolve, world, api) = plugin_world(&wit);
    let bytes = dummy_component(&resolve, world);

    let limits = PluginLimits::default();
    let (_root, package) = package(&api, &bytes, &limits);
    let engine = engine(&limits).expect("engine");
    let compiled = compile_package(&engine, &package, &limits).expect("the bytes compile");
    let linker = linker::<Services>(&engine).expect("linker");

    let mut startup = PluginStartup::instantiate(&linker, compiled.component(), Services, limits)
        .expect("a component of the contract is admitted");

    // Every exported body of the dummy module is `unreachable`, so the guest
    // faults in its first callback. That is a guest's fault and not the package's:
    // admitting the component was correct, and the fault costs the startup store,
    // which the host drops either way.
    let error = startup
        .configure("")
        .expect_err("the dummy module's only body is `unreachable`");
    assert!(
        matches!(error, HostError::Trap(_)),
        "a faulting guest is reported as trapped, not as a broken package: {error}"
    );
    assert!(
        format!("{error}").contains("unreachable"),
        "the trap is the guest's own body: {error}"
    );
    // A failed startup phase costs the store it ran in and nothing else: the
    // runtime store of the same component is still admitted, because the two
    // phases never share one.
    PluginInstance::instantiate(&linker, compiled.component(), Services, limits)
        .expect("a failed startup phase does not poison the runtime store");
}
