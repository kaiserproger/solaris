//! One deployed component package on disk: `plugin.toml` + its `plugin.wasm`.
//!
//! The manifest is parsed into the repository's existing plugin-manifest
//! contract (`mc_script::ScriptPluginManifest`) and validated *for the component
//! API version*, so a component package and a Luau package share every rule
//! about ids, bounds, capabilities, routes and dependencies while requesting
//! their own contract version. No second schema is introduced here.
//!
//! Nothing in this module runs guest code: it resolves paths, reads bounded
//! files and answers the validated manifest plus the artifact bytes. A package
//! that escapes its own root, exceeds a bound, or names a capability that does
//! not exist is refused before a `Component` is ever compiled.

use std::path::{Path, PathBuf};

use mc_script::{
    COMPONENT_PLUGIN_API_VERSION, MAX_PLUGIN_MANIFEST_BYTES, ScriptPluginManifest,
    ValidatedScriptPluginManifest,
};
use serde::Deserialize;

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
    /// Player command roots the package claims.
    #[serde(default)]
    pub player_commands: Vec<String>,
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
}

/// One package ready to be compiled: its validated manifest and artifact bytes.
#[derive(Debug)]
pub struct LoadedPackage {
    manifest: ValidatedScriptPluginManifest,
    requested_capabilities: Vec<String>,
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
    // The component contract's own version: a Luau package keeps requesting the
    // Luau contract until the last migration stage.
    let manifest = manifest
        .validate_for(COMPONENT_PLUGIN_API_VERSION)
        .map_err(|error| PackageError::Manifest {
            path: manifest_path.clone(),
            message: format!("{error:?}"),
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
