//! The host runtime: one thread that owns the endpoint, the instances and the
//! mapping from the server's events to the contract's events.
//!
//! The server keeps producing `ScriptEvent`s on the boundary it already owns; this
//! loop is the only place a guest is called. It maps each event to the contract's
//! own shape, delivers it to the instances that subscribed, converts what they
//! answer back into server commands and submits those through the same admission
//! every other plugin command takes. A guest that traps, runs out of budget or
//! answers something invalid is retired *and* loses its command routes, so a dead
//! instance cannot keep answering with a stale registration.
//!
//! No package's phases share a store. `configure` runs first, in a store of its
//! own that is dropped before the runtime store exists; `init` and every callback
//! after it run in the runtime store the host keeps. The deployment's own
//! contributions - the package catalog core reads, the client bundles, and the two
//! worldgen profiles - are settled before the first guest runs, so a deployment
//! refused for two owners of one worldgen profile never half-starts.
//!
//! One pushed simulation tick is not an event of this contract and is not delivered
//! as one: it is the clock of the timers each instance owns, which only the host
//! can schedule, fire and cancel, and the loop drives those here, under the same
//! discipline every other callback takes.

use std::collections::{BTreeSet, VecDeque};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mc_script::precommit::{
    DamageContext, DamageTarget, HookActor, HookDecision, HookFailure, HookFailurePolicy, Request,
};

use mc_script::{
    ClientBundle, CommandBatchError, HostCommandAdmission, PluginSettlementPlan,
    PluginWorldgenOreProfile, ScriptBatchSubmissionError, ScriptBoundary, ScriptEvent,
    ScriptEventKind, ScriptHostEndpoint, ScriptHostInput, ScriptHostInputSender,
    ScriptInventoryClick, ScriptOperationFailure, ScriptOperationOutcome, ScriptOperationPayload,
    ScriptOperationState, ScriptPlayerId, ScriptPlayerTeleportFailure, ScriptPluginStorageFailure,
    ScriptReloadCommitError, ScriptStorageChange, ValidatedScriptPluginManifest,
    script_boundary_pair,
};

use crate::adapter::{AdapterError, PlayerSessions, to_script_batch};
use crate::bindings::exports::solaris::plugin::events::{
    ChatSent, CommandInvoked, Event, EventContext, InventoryClick, InventoryMenuClicked,
    InventoryStorageOutcome, InventoryStorageTransactionAnswered, OnlinePlayersAnswered,
    OperationAnswered, OperationCommitted, OperationOutcome, OperationPayload, OperationRefused,
    PlayerJoined, PlayerLeft, PlayerSnapshot, PlayerTeleportAnswered, PlayerTeleportFailure,
    PlayerTeleportOutcome, PlayerZoneTransition, StorageCasAnswered, StorageGetAnswered,
    TimerFired, ZoneCommandAnswered, ZoneCommandOutcome,
};
use crate::bindings::exports::solaris::plugin::lifecycle::InitContext;
use crate::bindings::solaris::plugin::operation_types::OperationFailure;
use crate::bindings::solaris::plugin::storage::{
    StorageCasOutcome, StorageChange, StorageFailure, StorageGetOutcome, StorageRecord,
};
use crate::bindings::solaris::plugin::types::{LogLevel, Position};
use crate::discovery::DiscoveryError;
use crate::package::LoadedPackage;
use crate::startup::{DeploymentContribution, DeploymentSurface, aggregate_deployment};
use crate::timers::{self, MAX_TIMER_CALLBACKS_PER_TICK, TimerRefusal, TimerSchedule};
use crate::{
    CommandBatch, EpochTicker, HostError, HostServices, PluginInstance, PluginLimits, PluginStartup,
};

/// Bounded capacities of one host: the same queues the boundary enforces, chosen
/// here so a deployment states them once.
#[derive(Debug, Clone, Copy)]
pub struct HostQueues {
    pub events: usize,
    pub commands: usize,
}

impl Default for HostQueues {
    fn default() -> Self {
        Self {
            events: 1024,
            commands: 256,
        }
    }
}

/// How often the epoch watchdog advances. One tick is the resolution of every
/// callback deadline: a guest that blocks past its deadline is interrupted at the
/// next tick, whatever it is doing.
pub const EPOCH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

const CALLBACK_LATENCY_SAMPLE_CAPACITY: usize = 1_200;

/// Percentiles from the bounded most-recent callback sample window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CallbackLatencyPercentiles {
    pub samples: u64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
}

#[derive(Debug)]
struct CallbackLatencyWindow {
    samples: VecDeque<u64>,
}

impl Default for CallbackLatencyWindow {
    fn default() -> Self {
        Self {
            samples: VecDeque::with_capacity(CALLBACK_LATENCY_SAMPLE_CAPACITY),
        }
    }
}

impl CallbackLatencyWindow {
    fn record(&mut self, elapsed: Duration) {
        if self.samples.len() == CALLBACK_LATENCY_SAMPLE_CAPACITY {
            self.samples.pop_front();
        }
        self.samples.push_back(
            elapsed
                .as_micros()
                .min(u128::from(u64::MAX))
                .try_into()
                .expect("bounded microseconds fit u64"),
        );
    }

    fn snapshot(&self) -> CallbackLatencyPercentiles {
        let mut samples = self.samples.iter().copied().collect::<Vec<_>>();
        samples.sort_unstable();
        let Some(&max_us) = samples.last() else {
            return CallbackLatencyPercentiles::default();
        };
        let percentile = |percent: usize| {
            let rank = (percent * samples.len()).div_ceil(100);
            samples[rank.saturating_sub(1)]
        };
        CallbackLatencyPercentiles {
            samples: samples.len() as u64,
            p50_us: percentile(50),
            p95_us: percentile(95),
            p99_us: percentile(99),
            max_us,
        }
    }
}

/// Operator diagnostics of one hosted instance.
#[derive(Debug, Default)]
pub struct InstanceDiagnostics {
    /// Every guest `on-events` callback attempted by the runtime store.
    pub calls: u64,
    pub events_delivered: u64,
    pub commands_submitted: u64,
    pub commands_refused: u64,
    pub callback_latency: CallbackLatencyPercentiles,
}

/// Services a hosted guest gets: `tracing` under the plugin's own target, and the
/// id the host bound to it.
struct Tracing {
    id: String,
}

impl HostServices for Tracing {
    fn log(&mut self, level: LogLevel, message: &str) {
        match level {
            LogLevel::Error => tracing::error!(plugin = %self.id, "{message}"),
            LogLevel::Warn => tracing::warn!(plugin = %self.id, "{message}"),
            LogLevel::Info => tracing::info!(plugin = %self.id, "{message}"),
            LogLevel::Debug => tracing::debug!(plugin = %self.id, "{message}"),
            LogLevel::Trace => tracing::trace!(plugin = %self.id, "{message}"),
        }
    }

    fn plugin_id(&self) -> &str {
        &self.id
    }
}

/// One instance this host runs, with what it is allowed to answer.
struct Hosted<S: HostServices + 'static> {
    id: String,
    subscriptions: BTreeSet<String>,
    /// Player command roots this package claimed. A command is delivered to the
    /// package that owns its root, exactly as the server's own router does; it is
    /// not a subscription, so a package cannot receive commands it never declared.
    commands: BTreeSet<String>,
    admission: HostCommandAdmission,
    instance: PluginInstance<S>,
    diagnostics: InstanceDiagnostics,
    callback_latency: CallbackLatencyWindow,
    limits: PluginLimits,
    /// The timers this package owns, and the simulation tick it last observed.
    ///
    /// Both are the host's memory, not the server's: the schedule is this
    /// instance's own, and a retired instance never advances either again. The
    /// observed tick only moves forward, whatever order the server's events arrive
    /// in and whatever the server coalesced, so a stale tick can neither fire a
    /// timer early nor have a tick already delivered fire again.
    timers: TimerSchedule,
    observed_tick: u64,
    /// Set once a callback failed: the instance keeps its place for the final
    /// report, but it is never called again and it holds no routes.
    retired: bool,
}

impl<S: HostServices + 'static> Hosted<S> {
    fn record_callback(&mut self, started: Instant) {
        self.diagnostics.calls += 1;
        self.callback_latency.record(started.elapsed());
    }
}

/// Why a host could not start.
#[derive(Debug, thiserror::Error)]
pub enum HostStartError {
    /// The deployment was refused.
    #[error(transparent)]
    Deployment(#[from] DiscoveryError),
    /// The engine could not be built.
    #[error("host engine: {0}")]
    Engine(String),
    /// A package could not be compiled or started.
    #[error("plugin {id:?} failed to start: {message}")]
    Package { id: String, message: String },
    /// Two packages of one deployment declared the same worldgen profile.
    ///
    /// A world opens against exactly one ore profile and one settlement plan, so
    /// a second declaration has no second slot. It is refused before any guest
    /// runs rather than resolved by keeping the later declaration. `kind` is the
    /// profile the two packages both declared.
    #[error("plugins {first} and {second} both declare a worldgen {kind} profile")]
    WorldgenConflict {
        kind: &'static str,
        first: String,
        second: String,
    },
}

/// The restart-only part of a component deployment's contract.
///
/// These values belong to the server's already-open world and routing boundary,
/// so changing one needs a full server restart rather than an in-place guest swap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginReloadContractField {
    PluginIdentities,
    PackageCatalog,
    CommandAndChannelOwnership,
    CommandGrants,
    PrecommitRegistrations,
    DeploymentSurface,
    StartupContribution,
}

impl std::fmt::Display for PluginReloadContractField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::PluginIdentities => "ordered plugin identities",
            Self::PackageCatalog => "package catalog",
            Self::CommandAndChannelOwnership => "command and channel ownership",
            Self::CommandGrants => "effective command grants",
            Self::PrecommitRegistrations => "pre-commit registrations",
            Self::DeploymentSurface => "deployment surface",
            Self::StartupContribution => "startup contribution",
        })
    }
}

/// The outcome of replacing one live component generation.
#[derive(Debug)]
pub struct PluginReloadReport {
    /// Number of packages in the newly active generation.
    pub loaded_packages: usize,
    /// Final diagnostics from the generation retired after the switch.
    pub replaced: Vec<(String, InstanceDiagnostics)>,
}

#[derive(Debug, thiserror::Error)]
pub enum PluginReloadError {
    /// The host has stopped, or stopped while the request waited in its FIFO.
    #[error("plugin host is closed")]
    HostClosed,
    /// The live and candidate stores together exceed the static reload budget.
    #[error(
        "reload candidate requires {requested_bytes} guest-memory bytes, exceeding the {limit_bytes}-byte budget"
    )]
    CandidateMemory {
        requested_bytes: usize,
        limit_bytes: usize,
    },
    /// The candidate changes a contract that the running server has already used.
    #[error("reload requires restart: {field} changed")]
    RestartContractChanged { field: PluginReloadContractField },
    /// A candidate package could not be fully configured, instantiated, or initialized.
    #[error("reload candidate setup refused: {message}")]
    CandidateSetup { message: String },
    /// The boundary refused the otherwise-ready candidate atomically.
    #[error("reload commit refused: {message}")]
    CommitRefused { message: String },
}

struct PluginReloadRequest {
    packages: Vec<LoadedPackage>,
    response: tokio::sync::oneshot::Sender<Result<PluginReloadReport, PluginReloadError>>,
}

#[derive(Clone)]
struct PluginReloadContract {
    identities: Vec<String>,
    catalog: Vec<mc_script::PluginPackage>,
    routes: Vec<(Vec<String>, Vec<String>, Vec<String>)>,
    grants: Vec<mc_script::CommandCapabilities>,
    precommit_hooks: Vec<mc_script::precommit::HookRegistration>,
    surface: DeploymentSurface,
    contribution: DeploymentContribution,
}

impl PluginReloadContract {
    fn from_deployment(
        manifests: &[ValidatedScriptPluginManifest],
        catalog: Vec<mc_script::PluginPackage>,
        precommit_hooks: Vec<mc_script::precommit::HookRegistration>,
        surface: DeploymentSurface,
        contribution: DeploymentContribution,
    ) -> Self {
        Self {
            identities: manifests
                .iter()
                .map(|manifest| manifest.plugin_id().to_owned())
                .collect(),
            catalog,
            routes: manifests
                .iter()
                .map(|manifest| {
                    (
                        manifest.player_command_roots().to_vec(),
                        manifest.operator_command_roots().to_vec(),
                        manifest
                            .custom_payload_channels()
                            .into_iter()
                            .map(str::to_owned)
                            .collect(),
                    )
                })
                .collect(),
            grants: manifests
                .iter()
                .map(ValidatedScriptPluginManifest::to_command_capabilities)
                .collect(),
            precommit_hooks,
            surface,
            contribution,
        }
    }

    fn incompatibility(&self, candidate: &Self) -> Option<PluginReloadContractField> {
        if self.identities != candidate.identities {
            return Some(PluginReloadContractField::PluginIdentities);
        }
        if self.catalog != candidate.catalog {
            return Some(PluginReloadContractField::PackageCatalog);
        }
        if self.routes != candidate.routes {
            return Some(PluginReloadContractField::CommandAndChannelOwnership);
        }
        if self.grants != candidate.grants {
            return Some(PluginReloadContractField::CommandGrants);
        }
        if self.precommit_hooks != candidate.precommit_hooks {
            return Some(PluginReloadContractField::PrecommitRegistrations);
        }
        if self.surface != candidate.surface {
            return Some(PluginReloadContractField::DeploymentSurface);
        }
        if self.contribution != candidate.contribution {
            return Some(PluginReloadContractField::StartupContribution);
        }
        None
    }
}

/// A running deployment.
pub struct PluginHost {
    boundary: ScriptBoundary,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Vec<(String, InstanceDiagnostics)>>>,
    reload_sender: ScriptHostInputSender,
    /// What each package's `configure` contributed, as the host recorded it while
    /// it started the deployment. The server reads it before it opens a world:
    /// startup rules are materialized by the world's owners, not by this host.
    contribution: DeploymentContribution,
    /// The deployment's own declarations - the client bundles and the two
    /// worldgen profiles - settled before the first guest ran.
    surface: DeploymentSurface,
    /// The watchdog of every store this host started. It lives exactly as long as
    /// the host does: dropping it earlier would leave every callback deadline
    /// armed against an epoch nobody advances.
    ticker: Option<EpochTicker>,
}

impl PluginHost {
    /// The boundary the server keeps: events go in, admitted commands come out.
    #[must_use]
    pub fn boundary(&self) -> &ScriptBoundary {
        &self.boundary
    }

    /// What every package of this deployment answered to `configure`.
    ///
    /// Per package it is the plugin id, the validated startup rules when the
    /// package declared a contribution the startup contract accepts, a refusal
    /// when it declared one the contract does not, and neither when it declared
    /// none: a caller that has to fail startup closed reads
    /// [`DeploymentContribution::refusal`], and a package with no contribution is
    /// not a failure.
    #[must_use]
    pub fn contribution(&self) -> &DeploymentContribution {
        &self.contribution
    }

    /// Every client bundle this deployment declared.
    ///
    /// The packages' own bundles, concatenated in the order the packages were
    /// started. Each one carries its owner, its validated artifact and hash, its
    /// loaders and the content kinds it declares; the Loader decides what to stage
    /// from them, under the permission each content kind needs.
    #[must_use]
    pub fn client_bundles(&self) -> &[ClientBundle] {
        self.surface.client_bundles()
    }

    /// The one ore profile this deployment declared, if any.
    #[must_use]
    pub fn worldgen_ore_profile(&self) -> Option<PluginWorldgenOreProfile> {
        self.surface.ore_profile()
    }

    /// The one settlement plan this deployment declared, if any.
    #[must_use]
    pub fn worldgen_settlement_plan(&self) -> Option<&PluginSettlementPlan> {
        self.surface.settlement_plan()
    }

    /// Build and atomically activate an already-discovered replacement deployment.
    ///
    /// The candidate crosses the same FIFO as events, so events admitted before
    /// this request reach the old generation and later events reach the replacement.
    /// Its stores, configuration and initialization are all owned by the host
    /// thread; callers never need a second script boundary or session view.
    pub async fn reload(
        &self,
        packages: Vec<LoadedPackage>,
    ) -> Result<PluginReloadReport, PluginReloadError> {
        let (response, received) = tokio::sync::oneshot::channel();
        self.reload_sender
            .send_reload(Box::new(PluginReloadRequest { packages, response }))
            .await
            .map_err(|_| PluginReloadError::HostClosed)?;
        received.await.map_err(|_| PluginReloadError::HostClosed)?
    }

    /// Stop the host, run every live guest's `shutdown` and answer the counters.
    ///
    /// The host reclaims what it owns whether or not a guest's `shutdown`
    /// succeeds: a plugin never keeps a route, a timer or a registration alive by
    /// failing to clean up.
    pub fn stop(mut self) -> Vec<(String, InstanceDiagnostics)> {
        self.stop.store(true, Ordering::Release);
        self.boundary.close_event_admission();
        let counters = self
            .thread
            .take()
            .and_then(|thread| thread.join().ok())
            .unwrap_or_default();
        // The watchdog stops with the host, never before it.
        self.ticker.take();
        counters
    }
}

/// Start every package of the deployment on its own instance.
///
/// The packages are started before this function returns, so a deployment that
/// cannot start fails the caller instead of half-running.
pub fn start_deployment(
    packages: Vec<LoadedPackage>,
    limits: PluginLimits,
    queues: HostQueues,
    sessions: Arc<dyn PlayerSessions + Send + Sync>,
) -> Result<PluginHost, HostStartError> {
    start_deployment_with(packages, limits, queues, sessions, |id: &str| Tracing {
        id: id.to_owned(),
    })
}

/// The same, with the contract's host-side imports supplied by the caller.
///
/// A guest spends fuel in its own code, so fuel bounds a guest that computes and
/// nothing else: only the epoch deadline ends a guest blocked inside a host call.
/// Tests hold a real guest inside `log` through this seam to prove the deadline is
/// live.
pub fn start_deployment_with<S, F>(
    packages: Vec<LoadedPackage>,
    limits: PluginLimits,
    queues: HostQueues,
    sessions: Arc<dyn PlayerSessions + Send + Sync>,
    services: F,
) -> Result<PluginHost, HostStartError>
where
    S: HostServices + 'static,
    F: Fn(&str) -> S + Send + 'static,
{
    let engine =
        crate::engine(&limits).map_err(|error| HostStartError::Engine(error.to_string()))?;
    let ticker = EpochTicker::start(engine.clone(), EPOCH_INTERVAL)
        .map_err(|error| HostStartError::Engine(format!("epoch watchdog: {error}")))?;
    let linker =
        crate::linker::<S>(&engine).map_err(|error| HostStartError::Engine(error.to_string()))?;
    let (mut boundary, mut endpoint) = script_boundary_pair(
        NonZeroUsize::new(queues.events.max(1)).expect("non-zero"),
        NonZeroUsize::new(queues.commands.max(1)).expect("non-zero"),
    );
    let reload_sender = boundary.host_input_sender();

    // The deployment's own declarations are settled before the first guest runs:
    // core reads the catalog through this boundary, the Loader stages the bundles,
    // and a world opens against exactly one ore profile and one settlement plan.
    // The aggregation is the same one `check_deployment` performs, so a duplicate
    // declaration is refused identically on both paths.
    let surface =
        aggregate_deployment(&packages).map_err(|conflict| HostStartError::WorldgenConflict {
            kind: conflict.kind,
            first: conflict.first,
            second: conflict.second,
        })?;
    let manifests = packages
        .iter()
        .map(|package| package.manifest().clone())
        .collect::<Vec<_>>();
    // The catalog core's authored-data subsystems read is the host's own
    // discovery result, published on the boundary before it is handed on: a
    // package's directory and required features are what discovery validated, and
    // nothing here re-reads the manifest.
    let catalog = packages
        .iter()
        .map(LoadedPackage::to_plugin_package)
        .collect::<Vec<_>>();
    boundary.set_deployed_packages(catalog.clone());

    let mut precommit_hooks = packages
        .iter()
        .flat_map(|package| package.precommit_registrations().iter().cloned())
        .collect::<Vec<_>>();
    precommit_hooks.sort_by(mc_script::precommit::by_roster_order);
    boundary
        .set_precommit_hooks(precommit_hooks.clone())
        .map_err(|error| HostStartError::Engine(format!("pre-commit roster: {error}")))?;

    let mut hosted = Vec::new();
    let mut contribution = DeploymentContribution::default();
    for package in packages {
        let id = package.manifest().plugin_id().to_owned();
        let refuse = |message: String| HostStartError::Package {
            id: id.clone(),
            message,
        };
        let batch_limit = NonZeroUsize::new(limits.commands_per_call.max(1)).expect("non-zero");
        let compiled = crate::package::compile_package(&engine, &package, &limits)
            .map_err(|error| refuse(error.to_string()))?;
        let admission = HostCommandAdmission::from_manifest(package.manifest());
        let config = crate::check::read_config(package.root(), &limits).map_err(&refuse)?;
        // The startup phase runs in a store of its own, and that store is dropped
        // here - before the runtime store below exists - so no state the guest
        // wrote while answering startup rules can reach `init`. The sequence is
        // the check's own, so the two paths cannot disagree about either phase.
        let configured = {
            let mut startup =
                PluginStartup::instantiate(&linker, compiled.component(), services(&id), limits)
                    .map_err(|error| refuse(error.to_string()))?;
            startup
                .configure(&config)
                .map_err(|error| refuse(error.to_string()))?
        };
        // The recorded contribution is this answer, not a second run of the
        // phase: a guest's `configure` is called exactly once per instance, and
        // the rules the server opens the world with have to be the rules this
        // instance answered.
        contribution.record(&id, configured.as_ref());
        if configured.is_some() {
            // Startup rules are materialized by the server, not by the host: P4
            // hands the contribution to the existing validators and `StartupData`.
            tracing::info!(plugin = %id, "startup contribution reported");
        }
        let mut instance =
            PluginInstance::instantiate(&linker, compiled.component(), services(&id), limits)
                .map_err(|error| refuse(error.to_string()))?;
        let subscriptions = package
            .manifest()
            .event_subscriptions()
            .iter()
            .map(|subscription| subscription.event_name().to_owned())
            .collect::<BTreeSet<_>>();
        let batch = instance
            .init(
                &config,
                InitContext {
                    plugin_id: id.clone(),
                    api_version: compiled.api_version().to_owned(),
                    world_fingerprint: String::new(),
                },
            )
            .map_err(|error| refuse(error.to_string()))?;
        let mut diagnostics = InstanceDiagnostics::default();
        // A package may schedule its first timers from `init`: those requests are
        // checked against the same bounds a callback's are, and they are applied to
        // this instance's own schedule before it observes any tick, so the deadline
        // is the delay itself - no tick has been pushed yet - and the first pushed
        // tick that reaches it fires it.
        let (batch, mutations) = batch
            .split_timers(0)
            .map_err(|error| refuse(timer_refusal(error)))?;
        // The timers `init` asked for are staged on a copy exactly as every later
        // answer's are, and this instance's own schedule starts with them.
        let timers = timers::stage(&TimerSchedule::default(), mutations)
            .map_err(|error| refuse(timer_refusal(error)))?
            .unwrap_or_default();
        // Nothing `init` asked for publishes before the whole opening preparation
        // has succeeded: the routes this instance answers on are registered and its
        // batch is admitted first, and the instance is kept only once both did. A
        // deployment refused here stops with neither the batch nor the staged
        // timers having reached the boundary.
        endpoint
            .register_plugin_routes(package.manifest())
            .map_err(|error| refuse(format!("{error:?}")))?;
        submit(
            &mut endpoint,
            &admission,
            batch,
            batch_limit,
            sessions.as_ref(),
            &mut diagnostics,
        )
        .map_err(|error| refuse(format!("{error:?}")))?;
        let commands = package
            .manifest()
            .player_command_roots()
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        hosted.push(Hosted {
            id,
            subscriptions,
            commands,
            admission,
            instance,
            diagnostics,
            callback_latency: CallbackLatencyWindow::default(),
            limits,
            timers,
            observed_tick: 0,
            retired: false,
        });
    }
    let reload_contract = PluginReloadContract::from_deployment(
        &manifests,
        catalog,
        precommit_hooks.clone(),
        surface.clone(),
        contribution.clone(),
    );

    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let thread_sessions = Arc::clone(&sessions);
    let thread = std::thread::Builder::new()
        .name("mc-plugin-host".to_owned())
        .spawn(move || {
            let sessions = thread_sessions;
            let mut hosted = hosted;
            let mut tick = 0_u64;
            while !thread_stop.load(Ordering::Acquire) {
                let Some(input) = endpoint.recv_input_blocking() else {
                    break;
                };
                let event = match input {
                    ScriptHostInput::Event(event) => event,
                    ScriptHostInput::Reload(payload) => {
                        let Ok(request) = payload.downcast::<PluginReloadRequest>() else {
                            tracing::warn!("component host dropped control input it does not own");
                            continue;
                        };
                        let PluginReloadRequest { packages, response } = *request;
                        let candidate = match build_reload_candidate(
                            packages,
                            &engine,
                            &linker,
                            limits,
                            ReloadGeneration {
                                current_tick: tick,
                                contract: &reload_contract,
                            },
                            sessions.as_ref(),
                            &services,
                        ) {
                            Ok(candidate) => candidate,
                            Err(error) => {
                                let _ = response.send(Err(error));
                                continue;
                            }
                        };
                        let PluginReloadCandidate {
                            hosted: replacement,
                            manifests,
                            batches,
                        } = candidate;
                        let loaded_packages = replacement.len();
                        let mut replacement = Some(replacement);
                        let mut replaced = None;
                        match endpoint.commit_reload(&manifests, batches, || {
                            replaced = Some(std::mem::replace(
                                &mut hosted,
                                replacement
                                    .take()
                                    .expect("reload swap consumes the candidate generation"),
                            ));
                        }) {
                            Ok(()) => {
                                let replaced = shutdown_hosts(
                                    replaced.expect("a successful reload swaps the generation"),
                                );
                                let _ = response.send(Ok(PluginReloadReport {
                                    loaded_packages,
                                    replaced,
                                }));
                            }
                            Err(ScriptReloadCommitError::QueueClosed) => {
                                shutdown_hosts(
                                    replacement
                                        .take()
                                        .expect("a refused reload retains the candidate"),
                                );
                                let _ = response.send(Err(PluginReloadError::HostClosed));
                                break;
                            }
                            Err(ScriptReloadCommitError::QueueFull) => {
                                shutdown_hosts(
                                    replacement
                                        .take()
                                        .expect("a refused reload retains the candidate"),
                                );
                                let _ = response.send(Err(PluginReloadError::CommitRefused {
                                    message: "command queue is full".to_owned(),
                                }));
                            }
                            Err(ScriptReloadCommitError::Rejected { error }) => {
                                shutdown_hosts(
                                    replacement
                                        .take()
                                        .expect("a refused reload retains the candidate"),
                                );
                                let _ = response.send(Err(PluginReloadError::CommitRefused {
                                    message: format!("{error:?}"),
                                }));
                            }
                            Err(ScriptReloadCommitError::Ownership { error }) => {
                                shutdown_hosts(
                                    replacement
                                        .take()
                                        .expect("a refused reload retains the candidate"),
                                );
                                let _ = response.send(Err(PluginReloadError::CommitRefused {
                                    message: format!("{error:?}"),
                                }));
                            }
                            Err(error) => {
                                shutdown_hosts(
                                    replacement
                                        .take()
                                        .expect("a refused reload retains the candidate"),
                                );
                                let _ = response.send(Err(PluginReloadError::CommitRefused {
                                    message: format!("{error:?}"),
                                }));
                            }
                        }
                        continue;
                    }
                    ScriptHostInput::Precommit(request) => {
                        deliver_precommit(&mut hosted, &mut endpoint, request);
                        continue;
                    }
                    _ => continue,
                };
                // The tick a guest sees is the server's own simulation tick, taken
                // from the events that carry it. Counting deliveries instead would
                // make "every 20 ticks" mean twenty events.
                if let ScriptEventKind::ServerTick { tick: server_tick } = event.kind() {
                    tick = tick.max(*server_tick);
                    // A pushed simulation tick is the only clock a plugin's timers
                    // have. The tick is not an event of this contract - no
                    // subscription names it and no guest callback is called for it -
                    // so it reaches a guest only as the timers it made due, and
                    // every live instance is driven whether or not it subscribed to
                    // anything.
                    for hosted in &mut hosted {
                        if !hosted.retired {
                            deliver_timers(hosted, &mut endpoint, sessions.as_ref(), tick);
                        }
                    }
                    continue;
                }
                let Some((context, events)) = contract_events(&event, tick) else {
                    continue;
                };
                let name = event.event_name();
                let target = event.target_plugin_id();
                for hosted in &mut hosted {
                    if hosted.retired {
                        continue;
                    }
                    if let Some(target) = target {
                        if target != hosted.id {
                            continue;
                        }
                    } else if !interested(hosted, name, &event) {
                        continue;
                    }
                    deliver(hosted, &mut endpoint, sessions.as_ref(), context, &events);
                }
            }
            shutdown_hosts(hosted)
        })
        .map_err(|error| HostStartError::Engine(error.to_string()))?;

    Ok(PluginHost {
        boundary,
        stop,
        reload_sender,
        thread: Some(thread),
        contribution,
        surface,
        ticker: Some(ticker),
    })
}

struct PluginReloadCandidate<S: HostServices + 'static> {
    hosted: Vec<Hosted<S>>,
    manifests: Vec<ValidatedScriptPluginManifest>,
    batches: Vec<(HostCommandAdmission, mc_script::CommandBatch)>,
}

/// The active generation facts a staged candidate must preserve.
struct ReloadGeneration<'a> {
    current_tick: u64,
    contract: &'a PluginReloadContract,
}

fn ensure_reload_candidate_memory(
    limits: PluginLimits,
    active_packages: usize,
    candidate_packages: usize,
) -> Result<(), PluginReloadError> {
    let requested_bytes = active_packages
        .checked_add(candidate_packages)
        .and_then(|packages| packages.checked_mul(limits.memories))
        .and_then(|memories| memories.checked_mul(limits.guest_memory_bytes))
        .unwrap_or(usize::MAX);
    if requested_bytes > limits.reload_candidate_memory_bytes {
        return Err(PluginReloadError::CandidateMemory {
            requested_bytes,
            limit_bytes: limits.reload_candidate_memory_bytes,
        });
    }
    Ok(())
}

fn build_reload_candidate<S, F>(
    packages: Vec<LoadedPackage>,
    engine: &wasmtime::Engine,
    linker: &wasmtime::component::Linker<crate::InstanceState<S>>,
    limits: PluginLimits,
    active: ReloadGeneration<'_>,
    sessions: &dyn PlayerSessions,
    services: &F,
) -> Result<PluginReloadCandidate<S>, PluginReloadError>
where
    S: HostServices + 'static,
    F: Fn(&str) -> S,
{
    ensure_reload_candidate_memory(limits, active.contract.identities.len(), packages.len())?;
    let surface =
        aggregate_deployment(&packages).map_err(|error| PluginReloadError::CandidateSetup {
            message: error.to_string(),
        })?;
    let manifests = packages
        .iter()
        .map(|package| package.manifest().clone())
        .collect::<Vec<_>>();
    let catalog = packages
        .iter()
        .map(LoadedPackage::to_plugin_package)
        .collect::<Vec<_>>();
    let mut precommit_hooks = packages
        .iter()
        .flat_map(|package| package.precommit_registrations().iter().cloned())
        .collect::<Vec<_>>();
    precommit_hooks.sort_by(mc_script::precommit::by_roster_order);

    let mut hosted = Vec::with_capacity(packages.len());
    let mut batches = Vec::with_capacity(packages.len());
    let mut contribution = DeploymentContribution::default();
    for package in packages {
        let id = package.manifest().plugin_id().to_owned();
        let refuse = |message: String| PluginReloadError::CandidateSetup {
            message: format!("plugin {id:?}: {message}"),
        };
        let compiled = crate::package::compile_package(engine, &package, &limits)
            .map_err(|error| refuse(error.to_string()))?;
        let admission = HostCommandAdmission::from_manifest(package.manifest());
        let config = crate::check::read_config(package.root(), &limits).map_err(&refuse)?;
        let configured = {
            let mut startup =
                PluginStartup::instantiate(linker, compiled.component(), services(&id), limits)
                    .map_err(|error| refuse(error.to_string()))?;
            startup
                .configure(&config)
                .map_err(|error| refuse(error.to_string()))?
        };
        contribution.record(&id, configured.as_ref());

        let mut instance =
            PluginInstance::instantiate(linker, compiled.component(), services(&id), limits)
                .map_err(|error| refuse(error.to_string()))?;
        let subscriptions = package
            .manifest()
            .event_subscriptions()
            .iter()
            .map(|subscription| subscription.event_name().to_owned())
            .collect::<BTreeSet<_>>();
        let batch = instance
            .init(
                &config,
                InitContext {
                    plugin_id: id.clone(),
                    api_version: compiled.api_version().to_owned(),
                    world_fingerprint: String::new(),
                },
            )
            .map_err(|error| refuse(error.to_string()))?;
        let (batch, mutations) = batch
            .split_timers(active.current_tick)
            .map_err(|error| refuse(timer_refusal(error)))?;
        let timers = timers::stage(&TimerSchedule::default(), mutations)
            .map_err(|error| refuse(timer_refusal(error)))?
            .unwrap_or_default();
        let batch = to_script_batch(
            batch,
            batch_limit(&limits),
            sessions,
            admission.capabilities(),
        )
        .map_err(|error| refuse(format!("{error:?}")))?;
        let commands = package
            .manifest()
            .player_command_roots()
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        batches.push((admission.clone(), batch));
        hosted.push(Hosted {
            id,
            subscriptions,
            commands,
            admission,
            instance,
            diagnostics: InstanceDiagnostics::default(),
            callback_latency: CallbackLatencyWindow::default(),
            limits,
            timers,
            observed_tick: active.current_tick,
            retired: false,
        });
    }

    let candidate_contract = PluginReloadContract::from_deployment(
        &manifests,
        catalog,
        precommit_hooks,
        surface,
        contribution,
    );
    if let Some(field) = active.contract.incompatibility(&candidate_contract) {
        shutdown_hosts(hosted);
        return Err(PluginReloadError::RestartContractChanged { field });
    }
    Ok(PluginReloadCandidate {
        hosted,
        manifests,
        batches,
    })
}
fn shutdown_hosts<S: HostServices + 'static>(
    mut hosted: Vec<Hosted<S>>,
) -> Vec<(String, InstanceDiagnostics)> {
    for candidate in &mut hosted {
        if let Err(error) = candidate.instance.shutdown() {
            tracing::warn!(plugin = %candidate.id, %error, "shutdown failed");
        }
        candidate.diagnostics.callback_latency = candidate.callback_latency.snapshot();
    }
    hosted
        .into_iter()
        .map(|candidate| (candidate.id, candidate.diagnostics))
        .collect()
}

/// Whether one instance wants this event: by subscription, or - for a player
/// command - because it declared the root the player ran.
fn interested<S: HostServices + 'static>(
    hosted: &Hosted<S>,
    name: &str,
    event: &ScriptEvent,
) -> bool {
    match event.kind() {
        ScriptEventKind::PlayerCommand { root, .. } => hosted.commands.contains(root),
        _ => hosted.subscriptions.contains(name),
    }
}

/// Call one instance and submit what it answered.
///
/// The answer holds two kinds of request: the commands the server owns, which go
/// through the one admission every plugin command takes, and the timers this
/// instance owns, which are applied to its own schedule. Both are staged together
/// and both are published only when the whole answer was admitted, so a batch the
/// host refuses commits no timer of its own either, and a plugin cannot leave a
/// timer behind by answering one it cannot have.
///
/// A failure retires the instance and removes its routes: a guest that traps or
/// answers something invalid must not keep a registration that promises answers
/// it can no longer give. A retired instance is never called again, so the timers
/// it held never fire.
fn deliver<S: HostServices + 'static>(
    hosted: &mut Hosted<S>,
    endpoint: &mut ScriptHostEndpoint,
    sessions: &dyn PlayerSessions,
    context: EventContext,
    events: &[Event],
) {
    let started = Instant::now();
    let result = hosted.instance.on_events(context, events);
    hosted.record_callback(started);
    let batch = match result {
        Ok(batch) => batch,
        Err(error) => {
            // The guest answered with a `plugin-error`: that is a first-class
            // answer, not misbehaviour, so the batch is dropped and the instance
            // keeps serving. Only an instance the call itself retired (a trap, an
            // exhausted budget, an answer past a bound) loses its routes.
            if hosted.instance.retired_because().is_some() {
                retire(hosted, endpoint, &error);
            } else {
                tracing::debug!(plugin = %hosted.id, %error, "event refused by the plugin");
                hosted.diagnostics.commands_refused += 1;
            }
            return;
        }
    };
    hosted.diagnostics.events_delivered += events.len() as u64;
    let (batch, mutations) = match batch.split_timers(hosted.observed_tick) {
        Ok(split) => split,
        Err(error) => {
            // A timer request the contract cannot accept is the plugin's own
            // malformed answer, like a command the server's DTO refuses.
            retire(hosted, endpoint, &HostError::Answer(timer_refusal(error)));
            return;
        }
    };
    // The timers this answer asks for are staged on a copy of the instance's own
    // schedule, so an answer the host does not admit commits no timer either: a
    // refused batch leaves the plugin's timers exactly where they were.
    let staged = match timers::stage(&hosted.timers, mutations) {
        Ok(staged) => staged,
        Err(error) => {
            retire(hosted, endpoint, &HostError::Answer(timer_refusal(error)));
            return;
        }
    };
    if let Err(error) = submit(
        endpoint,
        &hosted.admission,
        batch,
        batch_limit(&hosted.limits),
        sessions,
        &mut hosted.diagnostics,
    ) {
        // A batch that could not be submitted is not a broken guest: queue
        // backpressure, a full admission ledger or a player who left between the
        // event and admission all drop the batch and leave the instance live.
        // Only a batch the *contract* refused (forged provenance, invalid DTO,
        // denied capability) is misbehaviour.
        match error {
            SubmissionFailure::Transient(message) => {
                tracing::debug!(plugin = %hosted.id, %message, "batch dropped, instance stays live");
                notify_batch_rejected(hosted, endpoint, context);
            }
            SubmissionFailure::Contract(message) => {
                retire(hosted, endpoint, &HostError::Answer(message));
            }
        }
        return;
    }
    if let Some(staged) = staged {
        hosted.timers = staged;
    }
}

/// Tell one component that the preceding batch was not admitted, giving it one
/// no-output turn to discard local correlations that named commands the boundary
/// never received.
fn notify_batch_rejected<S: HostServices + 'static>(
    hosted: &mut Hosted<S>,
    endpoint: &mut ScriptHostEndpoint,
    context: EventContext,
) {
    let started = Instant::now();
    let result = hosted
        .instance
        .on_events(context, &[Event::CommandBatchRejected]);
    hosted.record_callback(started);
    let batch = match result {
        Ok(batch) => batch,
        Err(error) => {
            if hosted.instance.retired_because().is_some() {
                retire(hosted, endpoint, &error);
            } else {
                tracing::debug!(plugin = %hosted.id, %error, "plugin refused batch-rejection notification");
                hosted.diagnostics.commands_refused += 1;
            }
            return;
        }
    };
    if !batch.into_commands().is_empty() {
        retire(
            hosted,
            endpoint,
            &HostError::Answer("batch-rejection notification returned commands".to_owned()),
        );
        return;
    }
    hosted.diagnostics.events_delivered += 1;
}

/// Drive one instance's own timers from one pushed simulation tick.
///
/// A pushed tick fires the timers whose deadline it reached - earliest deadline
/// first, then smallest timer id - up to the contract's bound of callbacks per
/// tick; the timers still due after that wait for a later pushed tick that moves
/// the clock, and keep the deadline they were scheduled for. A tick that does not
/// move the clock is the tick it repeats: it has already had its delivery, so it
/// fires nothing. Every callback of one delivery shares one allowance and one
/// staging batch, and nothing any of them changed publishes unless the whole
/// delivery and the admission of its commands succeeded: a guest must not multiply
/// its budget by making many timers due at once, and a plugin must not
/// half-reschedule itself because a later callback failed.
fn deliver_timers<S: HostServices + 'static>(
    hosted: &mut Hosted<S>,
    endpoint: &mut ScriptHostEndpoint,
    sessions: &dyn PlayerSessions,
    tick: u64,
) {
    // A pushed tick that does not move the clock this instance observes is not a
    // new tick: the value it repeats already had its delivery, so it fires nothing
    // and a timer the per-tick bound deferred waits for a tick that does move.
    // That is also why a stale tick - one the server pushed after a later one -
    // can never fire a timer of its own.
    if tick <= hosted.observed_tick {
        return;
    }
    hosted.observed_tick = tick;
    // Nothing due at this tick is the common case - every plugin that holds no
    // timer, and every pushed tick before one is due - and it costs no copy of the
    // schedule and no guest call at all.
    if hosted.timers.next_due(tick).is_none() {
        return;
    }
    // The schedule one delivery works on is a staging copy. What it takes, cancels
    // or adds becomes the instance's own schedule only once the delivery has been
    // admitted, so a refusal anywhere leaves every timer where it was and a later
    // pushed tick delivers it again.
    let mut staged = hosted.timers.clone();
    let Some(due) = staged.take_next_due(tick) else {
        return;
    };
    if let Err(error) = hosted.instance.arm_delivery() {
        retire(hosted, endpoint, &error);
        return;
    }
    let mut batch = CommandBatch::new();
    let mut due = Some(due);
    let mut delivered = 0_usize;
    let mut command_count = 0;
    while let Some((scheduled_tick, timer_id)) = due {
        let events = [Event::TimerFired(TimerFired {
            timer_id,
            scheduled_tick,
            fired_tick: tick,
        })];
        let context = EventContext {
            tick,
            first_sequence: 0,
            count: 1,
        };
        let started = Instant::now();
        let result = hosted.instance.on_events_within_delivery(context, &events);
        hosted.record_callback(started);
        let answer = match result {
            Ok(answer) => answer,
            Err(error) => {
                // A `plugin-error` is a first-class answer, not misbehaviour: the
                // delivery is dropped and the instance keeps serving, exactly as
                // for any other event. Only an instance the call itself retired (a
                // trap, an exhausted budget, an answer past a bound) loses its
                // routes, and a retired instance has no timers left to fire.
                if hosted.instance.retired_because().is_some() {
                    retire(hosted, endpoint, &error);
                } else {
                    tracing::debug!(plugin = %hosted.id, %error, "timer callback refused by the plugin");
                    hosted.diagnostics.commands_refused += 1;
                }
                return;
            }
        };
        delivered += 1;
        hosted.diagnostics.events_delivered += 1;
        // Timer mutations consume the same allowance as game commands. Count
        // before splitting them out, or each callback gets a fresh timer budget.
        command_count += answer.len();
        if command_count > hosted.limits.commands_per_call {
            retire(
                hosted,
                endpoint,
                &HostError::Answer("the pushed tick exceeded its command bound".to_owned()),
            );
            return;
        }
        let (answer, mutations) = match answer.split_timers(tick) {
            Ok(split) => split,
            Err(error) => {
                retire(hosted, endpoint, &HostError::Answer(timer_refusal(error)));
                return;
            }
        };
        for mutation in mutations {
            // The staged copy is the schedule the rest of this delivery reads, so
            // a timer an earlier callback cancelled cannot fire later in the same
            // pushed tick, and a deadline an earlier callback moved is the one the
            // next callback sees.
            if let Err(error) = timers::apply(&mut staged, mutation) {
                retire(hosted, endpoint, &HostError::Answer(timer_refusal(error)));
                return;
            }
        }
        if let Err(error) = batch.extend(answer.into_commands(), &hosted.limits) {
            // The commands of one pushed tick share the bound one callback's answer
            // has: many due timers must not answer with more host work than a
            // single callback could.
            let refusal = format!("the pushed tick exceeded its command bound: {error}");
            retire(hosted, endpoint, &HostError::Answer(refusal));
            return;
        }
        if delivered >= MAX_TIMER_CALLBACKS_PER_TICK {
            // The contract's bound per pushed tick: the timers still due arrive on
            // a later tick, with the deadline they were scheduled for.
            break;
        }
        due = staged.take_next_due(tick);
    }
    match submit(
        endpoint,
        &hosted.admission,
        batch,
        batch_limit(&hosted.limits),
        sessions,
        &mut hosted.diagnostics,
    ) {
        Ok(()) => hosted.timers = staged,
        // Backpressure, a full admission ledger or a player who left between the
        // callback and admission: nothing of this pushed tick publishes and the
        // instance stays live, so the timers it did not settle arrive again later.
        Err(SubmissionFailure::Transient(message)) => {
            tracing::debug!(plugin = %hosted.id, %message, "timer delivery dropped, instance stays live");
        }
        Err(SubmissionFailure::Contract(message)) => {
            retire(hosted, endpoint, &HostError::Answer(message));
        }
    }
}

/// The words an operator reads for a timer request the host refused.
///
/// The refusal names what the plugin got wrong - the id, the delay, the deadline
/// the delay adds up to, or the pending maximum - because it is the plugin's own
/// answer that broke a bound the contract states, not a state the host could
/// retry on the plugin's behalf.
fn timer_refusal(error: TimerRefusal) -> String {
    format!("timer request refused: {error}")
}

/// The command bound one submission enforces: the same count the staging area
/// already holds an answer to, never a second, larger number.
fn batch_limit(limits: &PluginLimits) -> NonZeroUsize {
    NonZeroUsize::new(limits.commands_per_call.max(1)).expect("non-zero")
}

fn retire<S: HostServices + 'static>(
    hosted: &mut Hosted<S>,
    endpoint: &mut ScriptHostEndpoint,
    error: &HostError,
) {
    tracing::warn!(plugin = %hosted.id, %error, "plugin instance retired after a failed callback");
    hosted.retired = true;
    hosted.timers = TimerSchedule::default();
    endpoint.unregister_plugin_routes(&hosted.id);
}

/// Why one callback's batch did not reach the server.
#[derive(Debug)]
enum SubmissionFailure {
    /// Nothing is wrong with the guest: the batch was refused for a state that
    /// can change - a full queue, a full admission ledger, a player who left.
    Transient(String),
    /// The guest's batch violated the contract.
    Contract(String),
}
/// Run the published roster as one ordered pre-commit chain.
///
/// Retired instances remain represented by their registrations: they answer a
/// handler failure under that registration's configured policy instead of being
/// silently removed from the protection the operator installed.
fn deliver_precommit<S: HostServices + 'static>(
    hosted: &mut [Hosted<S>],
    endpoint: &mut ScriptHostEndpoint,
    request: Request,
) {
    if request.is_expired() {
        let _ = request.fail(HookFailure::Expired);
        return;
    }
    match request.context() {
        mc_script::precommit::HookContext::Build(context) => {
            if !is_supported_hook_actor(context.actor()) {
                let _ = request.fail(HookFailure::Invalid);
                return;
            }
            for registration in request.hooks() {
                if Instant::now() >= request.deadline() {
                    let _ = request.fail(HookFailure::Expired);
                    return;
                }
                let result = hosted
                    .iter_mut()
                    .find(|candidate| candidate.id == registration.plugin_id())
                    .filter(|candidate| !candidate.retired)
                    .ok_or(())
                    .and_then(|candidate| {
                        candidate
                            .instance
                            .before_build(context, request.deadline())
                            .map_err(|error| {
                                if candidate.instance.retired_because().is_some() {
                                    retire(candidate, endpoint, &error);
                                }
                            })
                    });
                match result {
                    Ok(HookDecision::Keep) => {}
                    Ok(HookDecision::Cancel) => {
                        let _ = request.answer(HookDecision::Cancel);
                        return;
                    }
                    Ok(HookDecision::Replace(_)) | Err(())
                        if registration.on_failure() == HookFailurePolicy::Deny =>
                    {
                        let _ = request.answer(HookDecision::Cancel);
                        return;
                    }
                    Ok(HookDecision::Replace(_)) | Err(()) => {}
                    Ok(_) => {
                        let _ = request.answer(HookDecision::Cancel);
                        return;
                    }
                }
            }
            let _ = request.answer(HookDecision::Keep);
        }
        mc_script::precommit::HookContext::Damage(context) => {
            if !is_supported_hook_actor(context.source())
                || !is_supported_damage_target(context.target())
            {
                let _ = request.fail(HookFailure::Invalid);
                return;
            }
            let mut current = context.clone();
            for registration in request.hooks() {
                if Instant::now() >= request.deadline() {
                    let _ = request.fail(HookFailure::Expired);
                    return;
                }
                let result = hosted
                    .iter_mut()
                    .find(|candidate| candidate.id == registration.plugin_id())
                    .filter(|candidate| !candidate.retired)
                    .ok_or(())
                    .and_then(|candidate| {
                        candidate
                            .instance
                            .before_damage(&current, request.deadline())
                            .map_err(|error| {
                                if candidate.instance.retired_because().is_some() {
                                    retire(candidate, endpoint, &error);
                                }
                            })
                    });
                match result {
                    Ok(HookDecision::Keep) => {}
                    Ok(HookDecision::Cancel) => {
                        let _ = request.answer(HookDecision::Cancel);
                        return;
                    }
                    Ok(HookDecision::Replace(amount)) => {
                        current = DamageContext::try_new(
                            current.source().clone(),
                            current.target().clone(),
                            current.kind().to_owned(),
                            current.dimension().to_owned(),
                            current.position(),
                            amount,
                        )
                        .expect("a valid replacement preserves a valid damage context");
                    }
                    Ok(_) => {
                        let _ = request.answer(HookDecision::Cancel);
                        return;
                    }
                    Err(()) if registration.on_failure() == HookFailurePolicy::Deny => {
                        let _ = request.answer(HookDecision::Cancel);
                        return;
                    }
                    Err(()) => {}
                }
            }
            let decision = HookDecision::replacement(current.amount())
                .expect("a damage context always carries a valid amount");
            let _ = request.answer(decision);
        }
        _ => {
            let _ = request.fail(HookFailure::Invalid);
        }
    }
}

fn is_supported_hook_actor(actor: &HookActor) -> bool {
    matches!(
        actor,
        HookActor::Player(_) | HookActor::Entity(_) | HookActor::Plugin(_) | HookActor::Environment
    )
}

fn is_supported_damage_target(target: &DamageTarget) -> bool {
    matches!(target, DamageTarget::Player(_) | DamageTarget::Entity(_))
}

fn submit(
    endpoint: &mut ScriptHostEndpoint,
    admission: &HostCommandAdmission,
    batch: CommandBatch,
    batch_limit: NonZeroUsize,
    sessions: &dyn PlayerSessions,
    diagnostics: &mut InstanceDiagnostics,
) -> Result<(), SubmissionFailure> {
    let converted = match to_script_batch(batch, batch_limit, sessions, admission.capabilities()) {
        Ok(converted) => converted,
        Err(error) => {
            diagnostics.commands_refused += 1;
            return Err(match error {
                // A player who holds no session now: the command is refused, the
                // instance keeps running.
                AdapterError::UnknownPlayer => SubmissionFailure::Transient(format!("{error:?}")),
                // Everything else is the plugin's own answer being wrong: a
                // command past a bound the contract declares, a command needing a
                // capability the package never declared, a batch past the bound
                // the host itself enforces, or a command claiming host provenance.
                AdapterError::InvalidCommand { .. }
                | AdapterError::PermissionDenied { .. }
                | AdapterError::ProvenanceRejected
                | AdapterError::BatchRejected => SubmissionFailure::Contract(format!("{error:?}")),
            });
        }
    };
    let count = converted.commands().len() as u64;
    match endpoint.try_submit_plugin_batch(admission, converted) {
        Ok(()) => {
            diagnostics.commands_submitted += count;
            Ok(())
        }
        Err(error) => {
            diagnostics.commands_refused += count;
            // Queue backpressure and a full admission ledger are transient: the
            // batch is dropped and a later callback can retry. A forged command
            // or an unavailable admission cannot be retried into validity, and
            // only the contract's own refusals retire the instance.
            let transient = match &error {
                ScriptBatchSubmissionError::Full(_) | ScriptBatchSubmissionError::Closed(_) => true,
                ScriptBatchSubmissionError::Rejected { error, .. } => {
                    matches!(error, CommandBatchError::AdmissionUnavailable)
                }
                // The enum is `#[non_exhaustive]`: a refusal this host has not
                // been taught about is treated as backpressure rather than as the
                // package being broken, because retiring an instance and its
                // routes is the destructive answer and only the contract's own
                // refusals earn it. A new variant belongs in this match.
                _ => true,
            };
            let message = format!("{error:?}");
            Err(if transient {
                SubmissionFailure::Transient(message)
            } else {
                SubmissionFailure::Contract(message)
            })
        }
    }
}

/// Map one server event onto the contract's events.
///
/// Only the events the contract declares are mapped; anything else is skipped
/// rather than approximated, and the caller can see which names arrive by
/// subscribing to them.
fn contract_events(event: &ScriptEvent, tick: u64) -> Option<(EventContext, Vec<Event>)> {
    let mapped = match event.kind() {
        ScriptEventKind::PlayerJoined {
            player_id,
            username,
            context,
        } => vec![Event::PlayerJoined(PlayerJoined {
            player: context.uuid().to_owned(),
            session: session_of(*player_id),
            name: username.clone(),
        })],
        ScriptEventKind::PlayerLeft { player_id, .. } => vec![Event::PlayerLeft(PlayerLeft {
            session: session_of(*player_id),
        })],
        ScriptEventKind::PlayerChat {
            player_id,
            message,
            context,
        } => vec![Event::ChatSent(ChatSent {
            player: context.uuid().to_owned(),
            session: session_of(*player_id),
            message: message.clone(),
        })],
        ScriptEventKind::PlayerCommand {
            player_id,
            root,
            arguments,
            context,
            ..
        } => vec![Event::CommandInvoked(CommandInvoked {
            player: context.uuid().to_owned(),
            session: session_of(*player_id),
            username: context.username().to_owned(),
            operator: context.operator(),
            position: Position {
                x: context.x(),
                y: context.y(),
                z: context.z(),
            },
            name: root.clone(),
            arguments: arguments.split_whitespace().map(str::to_owned).collect(),
            raw_arguments: arguments.clone(),
        })],
        // The server's own snapshot of who is connected, renamed into the
        // contract's shape. The host neither re-derives nor re-orders it.
        ScriptEventKind::OnlinePlayersResult {
            request_id,
            players,
            truncated,
        } => vec![Event::OnlinePlayersAnswered(OnlinePlayersAnswered {
            request: request_id.clone(),
            players: players
                .iter()
                .map(|player| PlayerSnapshot {
                    player: player.context().uuid().to_owned(),
                    session: session_of(player.player_id()),
                    name: player.context().username().to_owned(),
                    dimension: player.dimension().to_owned(),
                    position: Position {
                        x: player.context().x(),
                        y: player.context().y(),
                        z: player.context().z(),
                    },
                })
                .collect(),
            truncated: *truncated,
        })],
        // The server's own typed answer to one admitted teleport, renamed the
        // same way: the reason a request did not commit is the server's, and a
        // failure the contract does not name yet is reported as the owner not
        // taking the commit, because the world did not change either way.
        ScriptEventKind::PlayerTeleportResult {
            request_id,
            player_id,
            position,
            failure,
        } => vec![Event::PlayerTeleportAnswered(PlayerTeleportAnswered {
            request: request_id.clone(),
            session: session_of(*player_id),
            position: Position {
                x: position.x(),
                y: position.y(),
                z: position.z(),
            },
            outcome: match failure {
                Some(failure) => PlayerTeleportOutcome::Refused(teleport_failure(*failure)),
                None => PlayerTeleportOutcome::Committed,
            },
        })],
        // The two storage answers are the contract's typed results: the plugin
        // reads what the server committed, never what it hoped it committed.
        ScriptEventKind::PluginStorageGetResult {
            request_id,
            value,
            version,
            failure,
            ..
        } => vec![Event::StorageGetAnswered(StorageGetAnswered {
            request: request_id.clone(),
            outcome: match failure {
                Some(failure) => StorageGetOutcome::Failed(storage_failure(*failure)),
                None => StorageGetOutcome::Read(StorageRecord {
                    value: value.clone(),
                    version: *version,
                }),
            },
        })],
        ScriptEventKind::PluginStorageCasResult {
            request_id,
            applied,
            version,
            failure,
            ..
        } => vec![Event::StorageCasAnswered(StorageCasAnswered {
            request: request_id.clone(),
            outcome: match failure {
                Some(failure) => StorageCasOutcome::Failed(storage_failure(*failure)),
                // The server reports a committed swap with the new revision and a
                // swap it did not apply without one; there is no third shape.
                None => match (*applied, *version) {
                    (true, Some(version)) => StorageCasOutcome::Committed(version),
                    _ => StorageCasOutcome::Refused,
                },
            },
        })],
        // The server's own typed answer to one operation this plugin issued,
        // renamed into the contract's event. The envelope is the plugin's request
        // id, the durable operation id it named and the outcome, and the durable id
        // is what a later `operation-status` addresses, so it is carried through
        // unchanged rather than re-derived from the request.
        ScriptEventKind::OperationResult {
            request_id,
            operation_id,
            outcome,
        } => match operation_answered(request_id, operation_id.as_deref(), outcome) {
            Some(answer) => vec![Event::OperationAnswered(answer)],
            None => return None,
        },
        // The server's zone owner answers one bit, and this is that bit in the
        // contract's own shape. It names the zone it acted on rather than a request,
        // so the answer is keyed by the zone and the host adds no correlation id the
        // server never sent - inventing one would promise an attribution the answer
        // cannot support when a plugin has two commands for one zone in flight. The
        // finer reasons behind a refusal are the zone owner's own vocabulary
        // (`ZoneAdapterError`, `ZoneCapacity` in `mc-net`), which never crosses the
        // script boundary, so the contract does not invent one here: a member is
        // added to `zone-command-outcome` by the change that carries a reason.
        ScriptEventKind::ZoneCommandResult { zone_id, accepted } => {
            vec![Event::ZoneCommandAnswered(ZoneCommandAnswered {
                zone: zone_id.clone(),
                outcome: if *accepted {
                    ZoneCommandOutcome::Applied
                } else {
                    ZoneCommandOutcome::Refused
                },
            })]
        }
        ScriptEventKind::PlayerZoneEntered {
            player_id,
            context,
            zone_id,
        } => vec![Event::PlayerZoneEntered(PlayerZoneTransition {
            player: context.uuid().to_owned(),
            session: session_of(*player_id),
            zone: zone_id.clone(),
        })],
        ScriptEventKind::PlayerZoneExited {
            player_id,
            context,
            zone_id,
        } => vec![Event::PlayerZoneExited(PlayerZoneTransition {
            player: context.uuid().to_owned(),
            session: session_of(*player_id),
            zone: zone_id.clone(),
        })],
        // The owners of one inventory and storage transaction answer one bit, and
        // this is that bit in the contract's own shape. The answer names the
        // plugin's correlation id and nothing else, as the command does: this
        // transaction has no durable operation id, and the reasons behind a
        // refusal stay with the owners that decided it.
        ScriptEventKind::InventoryStorageTransactionResult {
            request_id,
            committed,
        } => vec![Event::InventoryStorageTransactionAnswered(
            InventoryStorageTransactionAnswered {
                request: request_id.clone(),
                outcome: if *committed {
                    InventoryStorageOutcome::Committed
                } else {
                    InventoryStorageOutcome::Refused
                },
            },
        )],
        // One click on one of this plugin's own menu slots, in the contract's own
        // shape. The server's menu owner already addressed the event to the plugin
        // that opened the menu and normalized the click, so the host carries the
        // owner's own values through: the id and the slot index unchanged, and the
        // player and the session exactly as `command-invoked` takes them - the
        // identity from the context the click was built with, and the runtime id of
        // the connection it arrived on, not a lookup of whatever session the
        // identity holds now. A kind this contract does not name yet is dropped
        // rather than reported as one of the four it does: a plugin told "primary"
        // for a click the server decided was something else would act on a promise
        // the owner never made.
        ScriptEventKind::InventoryMenuClicked {
            player_id,
            context,
            menu_id,
            slot,
            click,
        } => match inventory_click(*click) {
            Some(click) => vec![Event::InventoryMenuClicked(InventoryMenuClicked {
                player: context.uuid().to_owned(),
                session: session_of(*player_id),
                menu: menu_id.clone(),
                slot: *slot,
                click,
            })],
            None => return None,
        },
        kind => vec![
            crate::world_events::map_event(kind)
                .or_else(|| crate::client_presentation::map_event(kind))?,
        ],
    };
    Some((
        EventContext {
            tick,
            first_sequence: 0,
            count: u32::try_from(mapped.len()).unwrap_or(u32::MAX),
        },
        mapped,
    ))
}

/// The server's storage failure, in the contract's own vocabulary.
///
/// The server's enum is `non_exhaustive`, and a failure the contract does not
/// name yet is reported as a durability failure: storage did not answer, and the
/// plugin must not read that as "the key holds nothing".
fn storage_failure(failure: ScriptPluginStorageFailure) -> StorageFailure {
    match failure {
        ScriptPluginStorageFailure::Unavailable => StorageFailure::Unavailable,
        ScriptPluginStorageFailure::DurabilityFailed | _ => StorageFailure::DurabilityFailed,
    }
}

/// The server's teleport failure, in the contract's own vocabulary.
///
/// The server's enum is `non_exhaustive`, and a reason the contract does not name
/// yet is reported as `runtime-unavailable`: the owner did not take the commit,
/// which is what a plugin has to know before it retries.
fn teleport_failure(failure: ScriptPlayerTeleportFailure) -> PlayerTeleportFailure {
    match failure {
        ScriptPlayerTeleportFailure::PlayerUnavailable => PlayerTeleportFailure::PlayerUnavailable,
        ScriptPlayerTeleportFailure::TeleportPending => PlayerTeleportFailure::TeleportPending,
        ScriptPlayerTeleportFailure::RuntimeUnavailable | _ => {
            PlayerTeleportFailure::RuntimeUnavailable
        }
    }
}

/// Preserve native correlation, including queries without durable ids and
/// per-member details on rejected operations.
fn operation_answered(
    request_id: &str,
    operation_id: Option<&str>,
    outcome: &ScriptOperationOutcome,
) -> Option<OperationAnswered> {
    let payload = operation_payload(outcome.payload())?;
    let outcome = match (outcome.state(), outcome.failure()) {
        (ScriptOperationState::Committed, None) => {
            OperationOutcome::Committed(OperationCommitted {
                revision: outcome.revision()?,
                payload,
            })
        }
        (ScriptOperationState::Rejected, Some(failure)) => {
            OperationOutcome::Refused(OperationRefused {
                reason: operation_failure(failure),
                payload,
            })
        }
        _ => return None,
    };
    Some(OperationAnswered {
        request: request_id.to_owned(),
        operation_id: operation_id.map(str::to_owned),
        outcome,
    })
}

fn operation_payload(payload: &ScriptOperationPayload) -> Option<OperationPayload> {
    Some(match payload {
        ScriptOperationPayload::None => OperationPayload::None,
        ScriptOperationPayload::StorageBatch { changes } => {
            OperationPayload::StorageBatch(changes.iter().map(storage_change).collect())
        }
        ScriptOperationPayload::Settlement { result } => {
            OperationPayload::Settlement(crate::domain_settlements::encode_result(result)?)
        }
        ScriptOperationPayload::Resident { result } => {
            OperationPayload::Resident(crate::domain_residents::encode_result(result)?)
        }
        ScriptOperationPayload::ResidentOrder { result } => {
            OperationPayload::ResidentOrder(crate::domain_residents::encode_order_result(result)?)
        }
        ScriptOperationPayload::OwnedInventory { result } => {
            OperationPayload::OwnedInventory(crate::domain_inventories::encode_result(result)?)
        }
        _ => return None,
    })
}

/// One click kind as the contract names it, or nothing when the contract has no
/// member for it.
///
/// The four members the contract carries are exactly the four the server can
/// decide today (`mc_script::ScriptInventoryClick`), and the host renames them
/// one for one. The server's enum is `non_exhaustive`, so a kind it starts
/// deciding later reaches this match as an unrecognized variant: it is left to the
/// slice that extends the contract, which is why the click is dropped instead of
/// being reported as one of the four - the plugin would act on a promise its menu
/// owner never made.
fn inventory_click(click: ScriptInventoryClick) -> Option<InventoryClick> {
    match click {
        ScriptInventoryClick::Primary => Some(InventoryClick::Primary),
        ScriptInventoryClick::Secondary => Some(InventoryClick::Secondary),
        ScriptInventoryClick::ShiftPrimary => Some(InventoryClick::ShiftPrimary),
        ScriptInventoryClick::ShiftSecondary => Some(InventoryClick::ShiftSecondary),
        _ => None,
    }
}

/// One key a committed batch changed, as the contract names it.
fn storage_change(change: &ScriptStorageChange) -> StorageChange {
    StorageChange {
        key: change.key.clone(),
        deleted: change.deleted,
    }
}

/// The server's own reason an operation did not commit, in the contract's
/// vocabulary.
///
/// Every member of the server's enum has a member here, so the reasons stay
/// distinguishable: a batch refused for a stale revision is not reported as one
/// refused for a reused operation id, and a lookup that found nothing is not
/// reported as a commit. The server's enum is `non_exhaustive`, and a reason this
/// contract does not name yet is reported as `unknown` rather than as one of the
/// reasons that mean something else: either way the operation did not commit.
pub(crate) fn operation_failure(failure: ScriptOperationFailure) -> OperationFailure {
    match failure {
        ScriptOperationFailure::InvalidRequest => OperationFailure::InvalidRequest,
        ScriptOperationFailure::Forbidden => OperationFailure::Forbidden,
        ScriptOperationFailure::StaleRevision => OperationFailure::StaleRevision,
        ScriptOperationFailure::NotFound => OperationFailure::NotFound,
        ScriptOperationFailure::Unloaded => OperationFailure::Unloaded,
        ScriptOperationFailure::Blocked => OperationFailure::Blocked,
        ScriptOperationFailure::InsufficientItems => OperationFailure::InsufficientItems,
        ScriptOperationFailure::Capacity => OperationFailure::Capacity,
        ScriptOperationFailure::Busy => OperationFailure::Busy,
        ScriptOperationFailure::RuntimeUnavailable => OperationFailure::RuntimeUnavailable,
        ScriptOperationFailure::OperationConflict => OperationFailure::OperationConflict,
        ScriptOperationFailure::CursorExpired => OperationFailure::CursorExpired,
        _ => OperationFailure::Unknown,
    }
}

/// The session a runtime player id names: the contract's `session-id` is exactly
/// the server's runtime id, so this is a rename, not a lookup.
fn session_of(player_id: ScriptPlayerId) -> u64 {
    player_id.value()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_root_change_requires_command_ownership_restart() {
        let active_manifest = mc_script::ScriptPluginManifest::new(
            "clock",
            "Clock",
            "0.1.0",
            mc_script::COMPONENT_PLUGIN_API_VERSION,
        )
        .declare_operator_command_root("clock_status")
        .validate()
        .unwrap();
        let candidate_manifest = mc_script::ScriptPluginManifest::new(
            "clock",
            "Clock",
            "0.2.0",
            mc_script::COMPONENT_PLUGIN_API_VERSION,
        )
        .declare_operator_command_root("clock_daytime")
        .validate()
        .unwrap();
        let active = PluginReloadContract::from_deployment(
            &[active_manifest],
            Vec::new(),
            Vec::new(),
            DeploymentSurface::default(),
            DeploymentContribution::default(),
        );
        let candidate = PluginReloadContract::from_deployment(
            &[candidate_manifest],
            Vec::new(),
            Vec::new(),
            DeploymentSurface::default(),
            DeploymentContribution::default(),
        );

        assert_eq!(
            active.incompatibility(&candidate),
            Some(PluginReloadContractField::CommandAndChannelOwnership)
        );
    }

    #[test]
    fn command_events_carry_server_operator_authority() {
        let event = ScriptEvent::try_player_command_with_context(
            "permissions",
            ScriptPlayerId::new(7),
            mc_script::ScriptPlayerContext::new(
                "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                "Ada",
                true,
                0.0,
                64.0,
                0.0,
            ),
            "perm",
            "set admin",
        )
        .expect("bounded server command event");

        let (_, events) = contract_events(&event, 42).expect("component event");
        let [Event::CommandInvoked(invoked)] = events.as_slice() else {
            panic!("expected one command event");
        };
        assert_eq!(invoked.player, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
        assert_eq!(invoked.session, 7);
        assert_eq!(invoked.username, "Ada");
        assert!(invoked.operator);
        assert_eq!(invoked.position.x, 0.0);
        assert_eq!(invoked.position.y, 64.0);
        assert_eq!(invoked.position.z, 0.0);
        assert_eq!(invoked.name, "perm");
        assert_eq!(invoked.arguments, ["set", "admin"]);
        assert_eq!(invoked.raw_arguments, "set admin");
    }

    #[test]
    fn callback_latency_window_reports_bounded_nearest_rank_percentiles() {
        let mut window = CallbackLatencyWindow::default();
        for micros in 0..=CALLBACK_LATENCY_SAMPLE_CAPACITY {
            window.record(Duration::from_micros(micros as u64));
        }

        assert_eq!(
            window.snapshot(),
            CallbackLatencyPercentiles {
                samples: CALLBACK_LATENCY_SAMPLE_CAPACITY as u64,
                p50_us: 600,
                p95_us: 1_140,
                p99_us: 1_188,
                max_us: 1_200,
            }
        );
    }
}
