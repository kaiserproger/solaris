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

use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use mc_script::{
    CommandBatchError, HostCommandAdmission, ScriptBatchSubmissionError, ScriptBoundary,
    ScriptEvent, ScriptEventKind, ScriptHostEndpoint, ScriptOperationFailure,
    ScriptOperationOutcome, ScriptOperationPayload, ScriptOperationState, ScriptPlayerId,
    ScriptPlayerTeleportFailure, ScriptPluginStorageFailure, ScriptStorageChange,
    script_boundary_pair,
};

use crate::adapter::{AdapterError, PlayerSessions, to_script_batch};
use crate::bindings::exports::solaris::plugin::events::{
    ChatSent, CommandInvoked, Event, EventContext, OnlinePlayersAnswered, OperationAnswered,
    OperationCommitted, OperationFailure, OperationOutcome, OperationPayload, PlayerJoined,
    PlayerLeft, PlayerSnapshot, PlayerTeleportAnswered, PlayerTeleportFailure,
    PlayerTeleportOutcome, StorageCasAnswered, StorageGetAnswered, ZoneCommandAnswered,
    ZoneCommandOutcome,
};
use crate::bindings::exports::solaris::plugin::lifecycle::InitContext;
use crate::bindings::solaris::plugin::storage::{
    StorageCasOutcome, StorageChange, StorageFailure, StorageGetOutcome, StorageRecord,
};
use crate::bindings::solaris::plugin::types::{LogLevel, Position};
use crate::discovery::DiscoveryError;
use crate::package::LoadedPackage;
use crate::startup::DeploymentContribution;
use crate::{CommandBatch, EpochTicker, HostError, HostServices, PluginInstance, PluginLimits};

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

/// Operator diagnostics of one hosted instance.
#[derive(Debug, Default)]
pub struct InstanceDiagnostics {
    pub calls: u64,
    pub events_delivered: u64,
    pub commands_submitted: u64,
    pub commands_refused: u64,
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
    batch_limit: usize,
    /// Set once a callback failed: the instance keeps its place for the final
    /// report, but it is never called again and it holds no routes.
    retired: bool,
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
}

/// A running deployment.
pub struct PluginHost {
    boundary: ScriptBoundary,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Vec<(String, InstanceDiagnostics)>>>,
    /// What each package's `configure` contributed, as the host recorded it while
    /// it started the deployment. The server reads it before it opens a world:
    /// startup rules are materialized by the world's owners, not by this host.
    contribution: DeploymentContribution,
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
    /// package declared a plan the startup contract accepts, a refusal when it
    /// declared one the contract does not, and neither when it declared no plan:
    /// a caller that has to fail startup closed reads
    /// [`DeploymentContribution::refusal`], and a package with no plan is not a
    /// failure.
    #[must_use]
    pub fn contribution(&self) -> &DeploymentContribution {
        &self.contribution
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
    F: Fn(&str) -> S,
{
    let engine =
        crate::engine(&limits).map_err(|error| HostStartError::Engine(error.to_string()))?;
    let ticker = EpochTicker::start(engine.clone(), EPOCH_INTERVAL)
        .map_err(|error| HostStartError::Engine(format!("epoch watchdog: {error}")))?;
    let linker =
        crate::linker::<S>(&engine).map_err(|error| HostStartError::Engine(error.to_string()))?;
    let (boundary, mut endpoint) = script_boundary_pair(
        NonZeroUsize::new(queues.events.max(1)).expect("non-zero"),
        NonZeroUsize::new(queues.commands.max(1)).expect("non-zero"),
    );

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
        let mut instance =
            PluginInstance::instantiate(&linker, compiled.component(), services(&id), limits)
                .map_err(|error| refuse(error.to_string()))?;
        let admission = HostCommandAdmission::from_manifest(package.manifest());
        let config = crate::check::read_config(package.root(), &limits).map_err(&refuse)?;
        let plan = instance
            .configure(&config)
            .map_err(|error| refuse(error.to_string()))?;
        // The recorded contribution is this answer, not a second run of the
        // phase: a guest's `configure` is called exactly once per instance, and
        // the rules the server opens the world with have to be the rules this
        // instance answered.
        contribution.record(&id, plan.as_ref());
        if plan.is_some() {
            // Startup rules are materialized by the server, not by the host: P4
            // hands the plan to the existing validators and `StartupData`.
            tracing::info!(plugin = %id, "startup rule plan reported");
        }
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
        submit(
            &mut endpoint,
            &admission,
            batch,
            batch_limit,
            sessions.as_ref(),
            &mut diagnostics,
        )
        .map_err(|error| refuse(format!("{error:?}")))?;
        endpoint
            .register_plugin_routes(package.manifest())
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
            batch_limit: limits.commands_per_call.max(1),
            retired: false,
        });
    }

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
                let Some(event) = endpoint.recv_event_blocking() else {
                    break;
                };
                // The tick a guest sees is the server's own simulation tick, taken
                // from the events that carry it. Counting deliveries instead would
                // make "every 20 ticks" mean twenty events.
                if let ScriptEventKind::ServerTick { tick: server_tick } = event.kind() {
                    tick = *server_tick;
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
            for hosted in &mut hosted {
                if let Err(error) = hosted.instance.shutdown() {
                    tracing::warn!(plugin = %hosted.id, %error, "shutdown failed");
                }
            }
            hosted
                .into_iter()
                .map(|hosted| (hosted.id, hosted.diagnostics))
                .collect()
        })
        .map_err(|error| HostStartError::Engine(error.to_string()))?;

    Ok(PluginHost {
        boundary,
        stop,
        thread: Some(thread),
        contribution,
        ticker: Some(ticker),
    })
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
/// A failure retires the instance and removes its routes: a guest that traps or
/// answers something invalid must not keep a registration that promises answers
/// it can no longer give.
fn deliver<S: HostServices + 'static>(
    hosted: &mut Hosted<S>,
    endpoint: &mut ScriptHostEndpoint,
    sessions: &dyn PlayerSessions,
    context: EventContext,
    events: &[Event],
) {
    let batch = match hosted.instance.on_events(context, events) {
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
    let batch_limit = NonZeroUsize::new(hosted.batch_limit).expect("non-zero");
    if let Err(error) = submit(
        endpoint,
        &hosted.admission,
        batch,
        batch_limit,
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
            }
            SubmissionFailure::Contract(message) => {
                retire(hosted, endpoint, &HostError::Answer(message));
            }
        }
    }
}

fn retire<S: HostServices + 'static>(
    hosted: &mut Hosted<S>,
    endpoint: &mut ScriptHostEndpoint,
    error: &HostError,
) {
    tracing::warn!(plugin = %hosted.id, %error, "plugin instance retired after a failed callback");
    hosted.retired = true;
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
            name: root.clone(),
            arguments: arguments.split_whitespace().map(str::to_owned).collect(),
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
        _ => return None,
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

/// One operation answer as the contract's own event, or nothing when the contract
/// cannot express it.
///
/// The envelope the contract fixes is the plugin's request id, the durable
/// operation id and the typed outcome. An answer that carries no durable id - one
/// from an operation family this contract does not name yet - is not delivered
/// rather than delivered with an invented id: every operation a component can
/// issue through this contract carries one. An outcome shape the contract does not
/// carry yet (another family's payload, or a state that is neither a commit nor a
/// refusal) is left to the slice that extends the union, which is why it is
/// dropped instead of being mapped onto a shape that means something else.
fn operation_answered(
    request_id: &str,
    operation_id: Option<&str>,
    outcome: &ScriptOperationOutcome,
) -> Option<OperationAnswered> {
    let operation_id = operation_id?;
    let outcome = match (outcome.state(), outcome.failure(), outcome.payload()) {
        (ScriptOperationState::Committed, _, ScriptOperationPayload::StorageBatch { changes }) => {
            OperationOutcome::Committed(OperationCommitted {
                revision: outcome.revision()?,
                payload: OperationPayload::StorageBatch(
                    changes.iter().map(storage_change).collect(),
                ),
            })
        }
        (ScriptOperationState::Rejected, Some(failure), _) => {
            OperationOutcome::Refused(operation_failure(failure))
        }
        _ => return None,
    };
    Some(OperationAnswered {
        request: request_id.to_owned(),
        operation_id: operation_id.to_owned(),
        outcome,
    })
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
fn operation_failure(failure: ScriptOperationFailure) -> OperationFailure {
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
