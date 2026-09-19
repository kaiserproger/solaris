//! One deployed component package on disk: `plugin.toml` + its `plugin.wasm`.
//!
//! The manifest is parsed into the repository's plugin-manifest contract
//! (`mc_script::ScriptPluginManifest`) and validated for the component API
//! version. One schema governs ids, bounds, capabilities, routes and
//! dependencies.
//!
//! Nothing in this module runs guest code: it resolves paths, reads bounded
//! files and answers the validated manifest plus the artifact bytes. A package
//! that escapes its own root, exceeds a bound, or names a capability that does
//! not exist is refused before a `Component` is ever compiled.
//!
//! The rest of the manifest is read by the component deployment parsers -
//! [`crate::client_bundle`] for `[client]`, [`crate::worldgen`] for `[worldgen]`,
//! [`crate::required_features`] for `required_features` - so one deployment
//! reaches canonical bundles, world generation profiles and feature declarations.
//! Every artifact a bundle names is read from inside the package and must match
//! its declared size and hash before the package is handed on, so a bundle that
//! cannot be delivered fails the package instead of the session.

use std::path::{Path, PathBuf};

use mc_script::{
    COMPONENT_PLUGIN_API_VERSION, ClientBundle, MAX_PLUGIN_MANIFEST_BYTES, PluginPackage,
    PluginSettlementPlan, PluginWorldgenOreProfile, ScriptPluginManifest,
    ValidatedScriptPluginManifest,
    precommit::{HookKind, HookRegistration},
};
use serde::Deserialize;

use crate::client_bundle::{self, DiskClient};
use crate::required_features;
use crate::worldgen::{self, DiskWorldgen, PackageWorldgen};
use crate::{HostError, PluginLimits};

/// The manifest every package must carry.
pub const MANIFEST_FILE: &str = "plugin.toml";
/// The component the manifest points at by default.
pub const DEFAULT_ENTRY_FILE: &str = "plugin.wasm";

/// `plugin.toml` as a component package writes it.
///
/// Unknown fields are refused: a typo in a capability name must fail the package
/// loudly instead of silently shipping a plugin with fewer rights than its author
/// asked for.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageManifest {
    /// The plugin id: also the durable identity its storage is keyed by.
    pub id: String,
    /// Human-readable name, for diagnostics.
    pub name: String,
    /// The package's own version, independent of the API version.
    pub version: String,
    /// The contract version this component was built against.
    pub api: String,
    /// Component file inside the package directory.
    #[serde(default = "default_entry")]
    pub entry: String,
    /// Event names the package wants delivered.
    #[serde(default)]
    pub events: Vec<String>,
    /// Capability names, using the contract's own vocabulary.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Feature-capability names whose authored data this package needs.
    ///
    /// Core reads this before a world opens, so it is the package's own
    /// declaration - not a duplicate of `capabilities` - and every
    /// feature-capability the manifest declares must be listed here too.
    #[serde(default)]
    pub required_features: Vec<String>,
    /// Player command roots the package claims.
    #[serde(default)]
    pub player_commands: Vec<String>,
    /// Pre-commit hooks this package implements, as it declares them.
    ///
    /// A declaration says which questions the package can answer and nothing
    /// else: exporting a hook in the component does not register it, and an
    /// operator registration for a hook the package does not declare is refused
    /// instead of being asked a question the package never claimed to answer.
    #[serde(default)]
    pub hooks: Vec<String>,
    /// Client bundles this package ships, validated against their artifacts.
    pub client: Option<DiskClient>,
    /// The world generation profiles this package declares.
    pub worldgen: Option<DiskWorldgen>,
}

fn default_entry() -> String {
    DEFAULT_ENTRY_FILE.to_owned()
}

/// Why a package directory was refused.
#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    /// A required file is missing or unreadable.
    #[error("package {path} could not be read: {message}")]
    Io { path: PathBuf, message: String },
    /// `plugin.toml` is not a valid manifest of this contract.
    #[error("package {path} has an invalid manifest: {message}")]
    Manifest { path: PathBuf, message: String },
    /// A declared path leaves the package directory or is not a plain file.
    #[error("package {path} declares {field} {value:?}, which is not a file inside the package")]
    Escape {
        path: PathBuf,
        field: &'static str,
        value: String,
    },
    /// The artifact is larger than a plugin artifact may be.
    #[error("package {path} carries a {bytes}-byte artifact, the bound is {bound}")]
    ArtifactTooLarge {
        path: PathBuf,
        bytes: usize,
        bound: usize,
    },
    /// A client bundle the manifest declares does not hold up.
    ///
    /// The message covers the whole declaration: an out-of-contract schema, a
    /// duplicate or unbounded field, a content kind without its permission, or an
    /// artifact whose path, size or SHA-256 disagrees with the manifest.
    #[error("package {path} declares an invalid client bundle: {message}")]
    ClientBundle { path: PathBuf, message: String },
}

/// One package ready to be compiled: its validated manifest and artifact bytes.
#[derive(Debug)]
pub struct LoadedPackage {
    manifest: ValidatedScriptPluginManifest,
    requested_capabilities: Vec<String>,
    required_features: Vec<String>,
    declared_hooks: Vec<HookKind>,
    precommit_registrations: Vec<HookRegistration>,
    client_bundles: Vec<ClientBundle>,
    worldgen: PackageWorldgen,
    artifact: Vec<u8>,
    root: PathBuf,
}

impl LoadedPackage {
    /// The validated manifest the host admits this plugin's commands under.
    #[must_use]
    pub fn manifest(&self) -> &ValidatedScriptPluginManifest {
        &self.manifest
    }

    /// The capability names the manifest requested, in declaration order.
    ///
    /// The operator's grants are checked against these names, so a package is
    /// never silently given fewer rights than it asked for.
    #[must_use]
    pub fn requested_capabilities(&self) -> &[String] {
        &self.requested_capabilities
    }

    /// The feature-capability names the manifest requires, in declaration order.
    ///
    /// This is what core reads to decide which authored-data subsystems a
    /// deployment needs - the settlement profile and the structure catalog among
    /// them - so it reaches the boundary exactly as the package declared it.
    #[must_use]
    pub fn required_features(&self) -> &[String] {
        &self.required_features
    }

    /// The pre-commit hooks the package declares it implements, in declaration
    /// order.
    ///
    /// A declaration is a capability claim and nothing more: it neither registers
    /// the package nor makes a question reach it, and a component that exports a
    /// hook it never declared is never asked that question.
    #[must_use]
    pub fn declared_hooks(&self) -> &[HookKind] {
        &self.declared_hooks
    }

    /// Whether the package declared it implements `kind`.
    #[must_use]
    pub fn declares_hook(&self, kind: HookKind) -> bool {
        self.declared_hooks.contains(&kind)
    }

    /// The operator registrations this package was authorized for, in the chain's
    /// own order.
    ///
    /// Discovery attaches these; a package loaded directly has none, because a
    /// registration is the operator's decision and not the package's. The host
    /// asks exactly these questions, in this order, and publishes them as the
    /// boundary's roster.
    #[must_use]
    pub fn precommit_registrations(&self) -> &[HookRegistration] {
        &self.precommit_registrations
    }

    /// Attach the operator registrations discovery authorized for this package.
    pub(crate) fn attach_precommit_registrations(&mut self, registrations: Vec<HookRegistration>) {
        self.precommit_registrations = registrations;
    }

    /// The client bundles this package ships, already hashed and read from disk.
    #[must_use]
    pub fn client_bundles(&self) -> &[ClientBundle] {
        &self.client_bundles
    }

    /// The ore profile this package declares, if any.
    #[must_use]
    pub const fn worldgen_ore_profile(&self) -> Option<PluginWorldgenOreProfile> {
        self.worldgen.ore_profile()
    }

    /// The settlement plan this package declares, if any.
    #[must_use]
    pub fn worldgen_settlement_plan(&self) -> Option<&PluginSettlementPlan> {
        self.worldgen.settlement_plan()
    }

    /// The deployment facts core needs about this package, for the boundary's
    /// deployed-package catalog: its id, its directory and its required features.
    #[must_use]
    pub fn to_plugin_package(&self) -> PluginPackage {
        PluginPackage::new(
            self.manifest.plugin_id(),
            self.root.clone(),
            self.required_features.clone(),
        )
    }

    /// The package's component bytes.
    #[must_use]
    pub fn artifact(&self) -> &[u8] {
        &self.artifact
    }

    /// The package directory, for diagnostics and authored data.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Load and validate the package in `root`, reading files within `limits`.
pub fn load_package(root: &Path, limits: &PluginLimits) -> Result<LoadedPackage, PackageError> {
    let manifest_path = root.join(MANIFEST_FILE);
    let text = read_bounded(&manifest_path, MAX_PLUGIN_MANIFEST_BYTES)?;
    let parsed: PackageManifest =
        toml::from_str(&text).map_err(|error| PackageError::Manifest {
            path: manifest_path.clone(),
            message: error.to_string(),
        })?;

    let api = ScriptPluginManifest::parse_api_version(&parsed.api).map_err(|_| {
        PackageError::Manifest {
            path: manifest_path.clone(),
            message: format!("api {:?} is not MAJOR.MINOR.PATCH", parsed.api),
        }
    })?;
    let mut manifest = ScriptPluginManifest::new(&parsed.id, &parsed.name, &parsed.version, api);
    for event in &parsed.events {
        manifest = manifest.subscribe_event(event);
    }
    for capability in &parsed.capabilities {
        manifest = manifest
            .declare_capability(capability)
            .map_err(|_| PackageError::Manifest {
                path: manifest_path.clone(),
                message: format!("unknown capability {capability:?}"),
            })?;
    }
    for root_command in &parsed.player_commands {
        manifest = manifest.declare_player_command_root(root_command);
    }
    let manifest = manifest
        .validate()
        .map_err(|error| PackageError::Manifest {
            path: manifest_path.clone(),
            message: format!("{error:?}"),
        })?;

    required_features::validate(&parsed.capabilities, &parsed.required_features).map_err(
        |message| PackageError::Manifest {
            path: manifest_path.clone(),
            message,
        },
    )?;
    // The declared hooks are the package's own claim about what it can answer.
    // Every name has to be a hook this contract has, and a package cannot declare
    // one twice: a duplicate would be a second registration of the same question
    // on the operator's side of the same package.
    let mut declared_hooks = Vec::new();
    for hook in &parsed.hooks {
        let kind = HookKind::parse(hook).ok_or_else(|| PackageError::Manifest {
            path: manifest_path.clone(),
            message: format!("unknown hook {hook:?}"),
        })?;
        if declared_hooks.contains(&kind) {
            return Err(PackageError::Manifest {
                path: manifest_path.clone(),
                message: format!("hook {hook:?} is declared twice"),
            });
        }
        declared_hooks.push(kind);
    }
    let worldgen = worldgen::materialize_worldgen(manifest.plugin_id(), parsed.worldgen).map_err(
        |message| PackageError::Manifest {
            path: manifest_path.clone(),
            message,
        },
    )?;
    let client_bundles =
        client_bundle::materialize_client_bundles(root, manifest.plugin_id(), parsed.client)
            .map_err(|message| PackageError::ClientBundle {
                path: root.to_path_buf(),
                message,
            })?;

    let artifact_path = resolve_inside(root, &parsed.entry, "entry")?;
    let metadata = std::fs::metadata(&artifact_path).map_err(|error| PackageError::Io {
        path: artifact_path.clone(),
        message: error.to_string(),
    })?;
    if !metadata.is_file() {
        return Err(PackageError::Escape {
            path: root.to_path_buf(),
            field: "entry",
            value: parsed.entry,
        });
    }
    let size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    if size > limits.artifact_bytes {
        return Err(PackageError::ArtifactTooLarge {
            path: root.to_path_buf(),
            bytes: size,
            bound: limits.artifact_bytes,
        });
    }
    let artifact = std::fs::read(&artifact_path).map_err(|error| PackageError::Io {
        path: artifact_path,
        message: error.to_string(),
    })?;
    Ok(LoadedPackage {
        manifest,
        requested_capabilities: parsed.capabilities,
        required_features: parsed.required_features,
        declared_hooks,
        // A package loaded directly carries no operator registration: who is
        // asked is the deployment's decision, which discovery makes.
        precommit_registrations: Vec::new(),
        client_bundles,
        worldgen,
        artifact,
        root: root.to_path_buf(),
    })
}

/// Compile a loaded package, refusing a component of another contract.
pub fn compile_package(
    engine: &wasmtime::Engine,
    package: &LoadedPackage,
    limits: &PluginLimits,
) -> Result<crate::CompiledPlugin, HostError> {
    crate::CompiledPlugin::compile(
        engine,
        package.artifact(),
        limits,
        &format!(
            "{}.{}.{}",
            COMPONENT_PLUGIN_API_VERSION.major(),
            COMPONENT_PLUGIN_API_VERSION.minor(),
            COMPONENT_PLUGIN_API_VERSION.patch()
        ),
    )
}

/// Resolve a manifest-declared path, refusing anything that is not a plain file
/// inside the package directory.
///
/// A package never chooses where the host reads from: `..`, absolute paths and
/// symlinks that leave the package root are all refused, and the check is on the
/// canonical path so a link cannot smuggle the read outside.
fn resolve_inside(root: &Path, value: &str, field: &'static str) -> Result<PathBuf, PackageError> {
    let escape = || PackageError::Escape {
        path: root.to_path_buf(),
        field,
        value: value.to_owned(),
    };
    if value.is_empty() || Path::new(value).is_absolute() {
        return Err(escape());
    }
    let canonical_root = root.canonicalize().map_err(|error| PackageError::Io {
        path: root.to_path_buf(),
        message: error.to_string(),
    })?;
    let candidate = root.join(value);
    let canonical = candidate.canonicalize().map_err(|_| escape())?;
    if !canonical.starts_with(&canonical_root) {
        return Err(escape());
    }
    Ok(canonical)
}

fn read_bounded(path: &Path, bound: usize) -> Result<String, PackageError> {
    let metadata = std::fs::metadata(path).map_err(|error| PackageError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    })?;
    if metadata.len() > bound as u64 {
        return Err(PackageError::Io {
            path: path.to_path_buf(),
            message: format!("file exceeds {bound} bytes"),
        });
    }
    std::fs::read_to_string(path).map_err(|error| PackageError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    })
}
