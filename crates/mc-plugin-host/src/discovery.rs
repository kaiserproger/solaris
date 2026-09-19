//! Discovery of a deployment: which packages run, and under which grants.
//!
//! A deployment is one directory of package directories. Strict mode is the
//! production contract: every filesystem entry must be a plugin directory, every
//! package must load, and the discovered id set must equal the configured
//! expected set exactly. Permissive mode exists for local iteration and skips an
//! ordinary broken package with a diagnostic - but never silently: the skipped
//! package is reported to the caller, and duplicate ids fail in both modes
//! because two packages of one id would share durable state.
//!
//! Grants are the operator's decision, not the package's: a capability the
//! manifest requests must be granted by `[plugins.grants.<id>]`, and a missing
//! grant fails the package instead of quietly shipping it with fewer rights.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use mc_script::precommit::{HookKind, HookRegistration};

use crate::PluginLimits;
use crate::package::{LoadedPackage, PackageError, load_package};

/// How a deployment directory is interpreted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryMode {
    /// Every entry must be a valid package and the id set must match `expected`.
    Strict,
    /// A package that fails to load is skipped with a diagnostic.
    Permissive,
}

/// The configured deployment: where packages live, how strictly they are read,
/// and which ids must be present.
#[derive(Debug, Clone)]
pub struct DeploymentConfig {
    pub root: PathBuf,
    pub mode: DiscoveryMode,
    /// The exact id set strict mode requires; empty means "no expectation".
    pub expected: Vec<String>,
    /// Operator grants per plugin id, from `[plugins.grants.<id>]`.
    pub grants: BTreeMap<String, Vec<String>>,
    /// Whether a capability without a grant fails the package. Strict production
    /// deployments set this; permissive ones may run without operator grants.
    pub require_grants: bool,
    /// Operator pre-commit hook registrations, from `[[plugins.hooks]]`: which
    /// package is asked which question, in which order, and what its failure
    /// means.
    ///
    /// These are the operator's decision and are independent of what a package
    /// declares: discovery refuses a registration no discovered package can serve
    /// - an unknown plugin, a hook the package does not declare, a duplicate
    ///   registration - in every mode, because a registration that silently did
    ///   nothing would be an operator believing an effect is held back when it is
    ///   not.
    pub precommit_hooks: Vec<HookRegistration>,
}

/// One package that was discovered but not admitted, with the reason.
#[derive(Debug)]
pub struct SkippedPackage {
    pub path: PathBuf,
    pub message: String,
}

/// The packages a deployment will run.
#[derive(Debug)]
pub struct DiscoveredDeployment {
    packages: Vec<LoadedPackage>,
    skipped: Vec<SkippedPackage>,
}

impl DiscoveredDeployment {
    /// The packages that loaded, sorted by plugin id.
    #[must_use]
    pub fn packages(&self) -> &[LoadedPackage] {
        &self.packages
    }

    /// The packages that were skipped, and why.
    #[must_use]
    pub fn skipped(&self) -> &[SkippedPackage] {
        &self.skipped
    }

    /// Take the packages for hosting.
    #[must_use]
    pub fn into_packages(self) -> Vec<LoadedPackage> {
        self.packages
    }

    /// Take the skipped diagnostics, leaving the package list in place.
    #[must_use]
    pub fn take_skipped(&mut self) -> Vec<SkippedPackage> {
        std::mem::take(&mut self.skipped)
    }
}

/// Why a deployment was refused.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    /// The deployment root could not be read.
    #[error("deployment root {path} could not be read: {message}")]
    Io { path: PathBuf, message: String },
    /// Strict mode found an entry that is not a package directory.
    #[error("{path} is not a plugin directory, and this deployment is strict")]
    NotAPackage { path: PathBuf },
    /// Duplicate plugin ids would share durable state.
    #[error("plugin id {id:?} is declared by more than one package")]
    DuplicateId { id: String },
    /// An expected package is absent.
    #[error("expected plugin {id:?} was not discovered")]
    ExpectedMissing { id: String },
    /// The expected set itself is malformed.
    #[error("the expected plugin set is malformed: {message}")]
    ExpectedSet { message: String },
    /// A discovered package was not expected.
    #[error("plugin {id:?} was discovered but is not in the expected set")]
    Unexpected { id: String },
    /// A package could not be loaded and the mode does not skip it.
    #[error("package {path} was refused: {error}")]
    Package { path: PathBuf, error: PackageError },
    /// A requested capability has no operator grant.
    #[error("plugin {id:?} requests capability {capability:?}, which is not granted")]
    Ungranted { id: String, capability: String },
    /// The operator's hook roster is malformed.
    #[error("the hook roster is malformed: {message}")]
    HookRoster { message: String },
    /// A registration names a package this deployment does not run.
    ///
    /// It covers both an id no package declares and a package permissive
    /// discovery skipped: an operator who configured a hook for a package expects
    /// that package to be asked, so a deployment that cannot run it fails instead
    /// of dropping the registration.
    #[error("hook registration names plugin {id:?}, which this deployment does not run")]
    HookPluginUnavailable { id: String },
    /// A registration names a hook the package never declared.
    #[error("plugin {id:?} is registered for {kind}, which its manifest does not declare")]
    UndeclaredHook { id: String, kind: HookKind },
    /// One package is registered twice for one hook.
    #[error("plugin {id:?} is registered twice for {kind}")]
    DuplicateHook { id: String, kind: HookKind },
}

/// Read the deployment directory and answer the packages it will run.
pub fn discover(
    config: &DeploymentConfig,
    limits: &PluginLimits,
) -> Result<DiscoveredDeployment, DiscoveryError> {
    let mut directories = Vec::new();
    let entries = std::fs::read_dir(&config.root).map_err(|error| DiscoveryError::Io {
        path: config.root.clone(),
        message: error.to_string(),
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| DiscoveryError::Io {
            path: config.root.clone(),
            message: error.to_string(),
        })?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| DiscoveryError::Io {
            path: path.clone(),
            message: error.to_string(),
        })?;
        if file_type.is_dir() {
            directories.push(path);
        } else if config.mode == DiscoveryMode::Strict {
            return Err(DiscoveryError::NotAPackage { path });
        }
    }
    directories.sort();

    let mut packages: Vec<LoadedPackage> = Vec::new();
    let mut skipped = Vec::new();
    for directory in directories {
        match load_package(&directory, limits) {
            Ok(package) => packages.push(package),
            Err(error) if config.mode == DiscoveryMode::Strict => {
                return Err(DiscoveryError::Package {
                    path: directory,
                    error,
                });
            }
            Err(error) => skipped.push(SkippedPackage {
                path: directory,
                message: error.to_string(),
            }),
        }
    }

    let mut seen = BTreeSet::new();
    for package in &packages {
        let id = package.manifest().plugin_id().to_owned();
        if !seen.insert(id.clone()) {
            return Err(DiscoveryError::DuplicateId { id });
        }
    }
    if config.mode == DiscoveryMode::Strict {
        let mut expected = BTreeSet::new();
        for id in &config.expected {
            if id.is_empty() {
                return Err(DiscoveryError::ExpectedSet {
                    message: "an expected plugin id is empty".to_owned(),
                });
            }
            if !expected.insert(id.as_str()) {
                return Err(DiscoveryError::ExpectedSet {
                    message: format!("duplicate expected plugin id {id:?}"),
                });
            }
        }
        let discovered: BTreeSet<&str> = seen.iter().map(String::as_str).collect::<BTreeSet<_>>();
        for id in &config.expected {
            if !discovered.contains(id.as_str()) {
                return Err(DiscoveryError::ExpectedMissing { id: id.clone() });
            }
        }
        for id in &discovered {
            if !config.expected.iter().any(|expected| expected == id) {
                return Err(DiscoveryError::Unexpected {
                    id: (*id).to_owned(),
                });
            }
        }
    }
    for package in &packages {
        check_grants(config, package)?;
    }
    attach_precommit_hooks(config, &mut packages)?;
    packages.sort_by(|left, right| {
        left.manifest()
            .plugin_id()
            .cmp(right.manifest().plugin_id())
    });
    Ok(DiscoveredDeployment { packages, skipped })
}

/// Validate the operator's hook registrations and attach each to its package.
///
/// The roster belongs to the deployment rather than to a package: one registration
/// is the operator's decision to ask one package one question, so it is attached to
/// that package and the host publishes the attached lists as the boundary's one
/// roster. Every rule here is checked in *both* modes, permissive included: a
/// registration no discovered package can serve is an operator believing an effect
/// is held back when it is not, which is exactly what must not pass silently. That
/// covers a package permissive discovery skipped, because a registration for it was
/// the operator asking for that package to run.
fn attach_precommit_hooks(
    config: &DeploymentConfig,
    packages: &mut [LoadedPackage],
) -> Result<(), DiscoveryError> {
    if config.precommit_hooks.is_empty() {
        return Ok(());
    }
    let mut attached: BTreeMap<String, Vec<HookRegistration>> = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for registration in &config.precommit_hooks {
        let id = registration.plugin_id();
        if id.is_empty() {
            return Err(DiscoveryError::HookRoster {
                message: "a hook registration names no plugin".to_owned(),
            });
        }
        if !seen.insert((id, registration.kind())) {
            return Err(DiscoveryError::DuplicateHook {
                id: id.to_owned(),
                kind: registration.kind(),
            });
        }
        let Some(package) = packages
            .iter()
            .find(|package| package.manifest().plugin_id() == id)
        else {
            return Err(DiscoveryError::HookPluginUnavailable { id: id.to_owned() });
        };
        if !package.declares_hook(registration.kind()) {
            return Err(DiscoveryError::UndeclaredHook {
                id: id.to_owned(),
                kind: registration.kind(),
            });
        }
        attached
            .entry(id.to_owned())
            .or_default()
            .push(registration.clone());
    }
    for package in packages {
        let id = package.manifest().plugin_id().to_owned();
        let mut registrations = attached.remove(&id).unwrap_or_default();
        registrations.sort_by(mc_script::precommit::by_roster_order);
        package.attach_precommit_registrations(registrations);
    }
    Ok(())
}

/// The capabilities one package may use: the ones it requests, all of which the
/// operator must have granted when this deployment requires grants.
fn check_grants(config: &DeploymentConfig, package: &LoadedPackage) -> Result<(), DiscoveryError> {
    if !config.require_grants {
        return Ok(());
    }
    let id = package.manifest().plugin_id();
    let granted = config.grants.get(id);
    for capability in package.requested_capabilities() {
        let allowed = granted.is_some_and(|granted| granted.iter().any(|it| it == capability));
        if !allowed {
            return Err(DiscoveryError::Ungranted {
                id: id.to_owned(),
                capability: capability.clone(),
            });
        }
    }
    Ok(())
}
