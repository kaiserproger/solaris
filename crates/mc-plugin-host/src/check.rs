//! `--check`: validate a whole deployment without touching a world.
//!
//! The check reads the same directory a run would, compiles every selected
//! component, and runs each plugin's startup phases - `configure` for its rule
//! plan and `init` for its opening commands - against a boundary whose command
//! queue nobody drains. Nothing here creates a world, writes storage, opens a
//! listener, or applies a command: a check that reports success claims exactly
//! the contract, the artifacts and the startup phases it exercised, and no more.

use std::path::Path;

use mc_script::{COMPONENT_PLUGIN_API_VERSION, GameplayRules, script_boundary_pair};
use std::num::NonZeroUsize;

use crate::adapter::{AdapterError, NoSessions, to_script_batch};
use crate::bindings::solaris::plugin::types::LogLevel;
use crate::discovery::{DeploymentConfig, DiscoveryError, SkippedPackage, discover};
use crate::package::{LoadedPackage, compile_package};
use crate::startup::convert_rule_plan;
use crate::{EpochTicker, HostServices, PluginInstance, PluginLimits};

/// How many commands one deployment check will look at before giving up on a
/// package: a check must not become an unbounded amount of host work.
const CHECK_COMMAND_QUEUE: usize = 64;
/// Events one queued batch may hold during a check.
const CHECK_EVENT_QUEUE: usize = 64;
/// Interval of the check's epoch watchdog.
const CHECK_EPOCH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// What one checked package answered.
#[derive(Debug)]
pub struct CheckedPackage {
    /// The plugin id the manifest declared.
    pub id: String,
    /// The contract version the package requested, as `MAJOR.MINOR.PATCH`.
    pub api: String,
    /// The startup rules the package's plan validated into, when it declares a
    /// plan.
    ///
    /// This is the run path's own conversion, not a second opinion: a check
    /// reports exactly the rules the server would open the world with, and a plan
    /// the startup contract refuses fails the check instead of being reported.
    pub plan: Option<GameplayRules>,
    /// How many commands the package's `init` staged and the boundary admitted.
    ///
    /// A check has no session registry, so a command that addresses a player by
    /// stable identity is not converted and not submitted: it is reported as
    /// unverifiable rather than counted as admitted.
    pub admitted_commands: usize,
    /// Whether the opening batch named a player a check cannot resolve.
    pub unverifiable_player_commands: bool,
}

/// The result of checking one deployment.
#[derive(Debug)]
pub struct CheckReport {
    checked: Vec<CheckedPackage>,
    skipped: Vec<SkippedPackage>,
}

impl CheckReport {
    /// The packages the check verified.
    #[must_use]
    pub fn checked(&self) -> &[CheckedPackage] {
        &self.checked
    }

    /// The packages the deployment skipped, and why.
    #[must_use]
    pub fn skipped(&self) -> &[SkippedPackage] {
        &self.skipped
    }
}

/// Why a deployment check failed.
#[derive(Debug, thiserror::Error)]
pub enum CheckError {
    /// The deployment itself could not be read or admitted.
    #[error(transparent)]
    Discovery(#[from] DiscoveryError),
    /// A package was refused by the contract or by its own startup phases.
    #[error("plugin {id:?} failed its check: {message}")]
    Package { id: String, message: String },
}

/// Services the check's throwaway stores get: capture the lines, own the id.
struct CheckServices {
    id: String,
    lines: Vec<(LogLevel, String)>,
}

impl HostServices for CheckServices {
    fn log(&mut self, level: LogLevel, message: &str) {
        self.lines.push((level, message.to_owned()));
    }

    fn plugin_id(&self) -> &str {
        &self.id
    }
}

/// Validate the deployment described by `config` without any game-side effect.
pub fn check_deployment(
    config: &DeploymentConfig,
    limits: &PluginLimits,
) -> Result<CheckReport, CheckError> {
    let mut deployment = discover(config, limits)?;
    let engine = crate::engine(limits).map_err(|error| CheckError::Package {
        id: "engine".to_owned(),
        message: error.to_string(),
    })?;
    let ticker = EpochTicker::start(engine.clone(), CHECK_EPOCH_INTERVAL).map_err(|error| {
        CheckError::Package {
            id: "engine".to_owned(),
            message: format!("epoch watchdog: {error}"),
        }
    })?;
    let mut checked = Vec::new();
    for package in deployment.packages() {
        checked.push(check_package(&engine, package, limits)?);
    }
    drop(ticker);
    Ok(CheckReport {
        checked,
        skipped: deployment.take_skipped(),
    })
}

fn check_package(
    engine: &wasmtime::Engine,
    package: &LoadedPackage,
    limits: &PluginLimits,
) -> Result<CheckedPackage, CheckError> {
    let id = package.manifest().plugin_id().to_owned();
    let refuse = |message: String| CheckError::Package {
        id: id.clone(),
        message,
    };
    let compiled =
        compile_package(engine, package, limits).map_err(|error| refuse(error.to_string()))?;
    let linker =
        crate::linker::<CheckServices>(engine).map_err(|error| refuse(error.to_string()))?;
    let mut instance = PluginInstance::instantiate(
        &linker,
        compiled.component(),
        CheckServices {
            id: id.clone(),
            lines: Vec::new(),
        },
        *limits,
    )
    .map_err(|error| refuse(error.to_string()))?;

    let config_text = read_config(package.root(), limits).map_err(refuse)?;
    let plan = instance
        .configure(&config_text)
        .map_err(|error| refuse(error.to_string()))?;
    // A check reports the plan the run path would materialize, through the same
    // conversion: a plan the startup contract refuses is a startup answer this
    // deployment cannot run with, so it fails the check rather than being handed
    // to the caller as though something would accept it.
    let plan = plan
        .as_ref()
        .map(convert_rule_plan)
        .transpose()
        .map_err(|refusal| refuse(refusal.to_string()))?;

    // `init` runs against a real boundary, so its commands pass the same
    // admission a live deployment would give them - but this check owns the only
    // receiver and never drains it, so no command is ever applied.
    let (boundary, endpoint) = script_boundary_pair(
        NonZeroUsize::new(CHECK_EVENT_QUEUE).expect("non-zero"),
        NonZeroUsize::new(CHECK_COMMAND_QUEUE).expect("non-zero"),
    );
    let admission = mc_script::HostCommandAdmission::from_manifest(package.manifest());
    let batch = instance
        .init(
            &config_text,
            crate::bindings::exports::solaris::plugin::lifecycle::InitContext {
                plugin_id: id.clone(),
                api_version: api_text(COMPONENT_PLUGIN_API_VERSION),
                world_fingerprint: String::new(),
            },
        )
        .map_err(|error| refuse(error.to_string()))?;
    // The batch is converted first: conversion is where a command that names a
    // player is resolved to a session, and a check has no session registry, so a
    // player-targeted command is counted as unverifiable here instead of being
    // invented or silently dropped.
    let converted = match to_script_batch(
        batch,
        NonZeroUsize::new(CHECK_COMMAND_QUEUE).expect("non-zero"),
        &NoSessions,
        admission.capabilities(),
    ) {
        Ok(converted) => Some(converted),
        Err(AdapterError::UnknownPlayer) => None,
        Err(error) => return Err(refuse(format!("{error:?}"))),
    };
    let admitted_commands = converted
        .as_ref()
        .map_or(0, |converted| converted.commands().len());
    let commands_needed_a_session = converted.is_none();
    if let Some(converted) = converted
        && let Err(error) = endpoint.try_submit_plugin_batch(&admission, converted)
    {
        return Err(refuse(format!("{error:?}")));
    }
    drop(boundary);
    let _ = instance.shutdown();

    Ok(CheckedPackage {
        id,
        api: compiled.api_version().to_owned(),
        plan,
        admitted_commands,
        unverifiable_player_commands: admitted_commands == 0 && commands_needed_a_session,
    })
}

/// The `MAJOR.MINOR.PATCH` text of one contract version.
fn api_text(version: mc_script::ScriptApiVersion) -> String {
    format!(
        "{}.{}.{}",
        version.major(),
        version.minor(),
        version.patch()
    )
}

/// The package's own `config.toml`, or an empty string when it carries none.
pub(crate) fn read_config(root: &Path, limits: &PluginLimits) -> Result<String, String> {
    let path = root.join("config.toml");
    match std::fs::metadata(&path) {
        Ok(metadata) if metadata.len() > limits.text_bytes as u64 => Err(format!(
            "config.toml is {} bytes, the bound is {}",
            metadata.len(),
            limits.text_bytes
        )),
        Ok(_) => std::fs::read_to_string(&path).map_err(|error| error.to_string()),
        Err(_) => Ok(String::new()),
    }
}
