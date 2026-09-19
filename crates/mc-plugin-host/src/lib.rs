//! The WASM (Wasmtime) plugin host.
//!
//! One plugin package becomes one component instance with its own `Store`. The
//! host owns every effect a plugin asks for: a callback returns a *staging*
//! batch of typed commands, the host validates the whole batch against the
//! plugin's grants and the configured budgets, and only then does the caller
//! hand it to the server's own admission boundary. Nothing in this crate touches
//! a world, a session or a storage backend - it converts between the
//! `solaris:plugin` component contract and the runtime-independent script types,
//! and it enforces the execution budget the plan requires before any guest code
//! can run (fuel, epoch deadline, memory, tables, instances and stack are all set
//! on the `Store`/`Engine` *before* instantiation, because a component's
//! initializers run guest code).
//!
//! Limits are deliberately measured in the units Wasmtime actually accounts:
//! fuel for guest instructions and a wall-clock epoch deadline as an independent
//! watchdog. They are component-host limits, not elapsed-millisecond budgets.
//!
//! One of them bounds the *host*: [`PluginLimits::hostcall_bytes`] is the guest's
//! transfer budget, spent by Wasmtime before it copies a string or a list element
//! out of guest memory. The store limiter cannot do that job - it bounds what the
//! guest allocates, not what lifting its answer allocates on the host's side -
//! and Wasmtime's own default for it is large enough that a hostile guest could
//! choose how much memory the host spends per callback.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder};

pub mod adapter;
pub mod check;
pub mod client_bundle;
mod client_presentation;
pub mod discovery;
mod domain_inventories;
mod domain_residents;
mod domain_settlements;
pub mod host;
pub mod instance;
pub mod limits;
pub mod package;
pub mod required_features;
pub mod staging;
pub mod startup;
mod timers;
mod world_events;
pub mod worldgen;

pub use adapter::{AdapterError, NoSessions, PlayerSessions, to_script_batch};
pub use check::{CheckError, CheckReport, CheckedPackage, check_deployment};
pub use discovery::{
    DeploymentConfig, DiscoveredDeployment, DiscoveryError, DiscoveryMode, discover,
};
pub use host::{
    HostQueues, HostStartError, InstanceDiagnostics, PluginHost, PluginReloadContractField,
    PluginReloadError, PluginReloadReport, start_deployment, start_deployment_with,
};
pub use instance::{PluginInstance, PluginStartup};
pub use limits::{PluginLimits, TRUNCATION_MARKER};
pub use package::{LoadedPackage, PackageError, PackageManifest, load_package};
pub use staging::{CommandBatch, StagingError};
pub use startup::{
    ContributionOutcome, ContributionRefusal, DeploymentContribution, FieldOverflow,
    PackageContribution, convert_startup_contribution,
};

/// Why a plugin could not be compiled, instantiated or called.
///
/// The variants separate a package that is wrong from a guest that misbehaved:
/// an operator fixes the first, and the second retires one instance rather than
/// the server.
#[derive(Debug, Clone, thiserror::Error)]
pub enum HostError {
    /// The package's bytes are not a component, or not a component of this
    /// contract. No guest code ran.
    #[error("component rejected: {0}")]
    Component(String),
    /// The engine could not be configured. A startup failure, not a package one.
    #[error("engine configuration failed: {0}")]
    Engine(String),
    /// Instantiation failed for a component that matched the contract, e.g. a
    /// missing export or a constant-expression trap.
    #[error("instantiation failed: {0}")]
    Instantiate(String),
    /// The guest trapped. The instance is unusable afterwards and is retired.
    #[error("guest trapped: {0}")]
    Trap(String),
    /// The guest ran out of fuel or outlived its epoch deadline. The instance is
    /// retired exactly like a trap.
    #[error("guest exceeded its execution budget")]
    Budget,
    /// The guest's answer violated the contract.
    #[error("guest answer rejected: {0}")]
    Answer(String),
}

/// One guest call's worth of fuel and the watchdog that interrupts it.
///
/// The epoch is advanced by a thread this type owns, never by the worker that is
/// running the guest: a worker blocked inside a guest call cannot also be the
/// clock that interrupts it.
#[derive(Debug)]
pub struct EpochTicker {
    running: Arc<AtomicBool>,
    ticks: Arc<AtomicU64>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl EpochTicker {
    /// Start ticking `engine`'s epoch every `interval`, and stop when dropped.
    ///
    /// A host that cannot start its watchdog must not run guests without one: the
    /// caller is told instead of silently losing every callback deadline.
    pub fn start(engine: Engine, interval: Duration) -> std::io::Result<Self> {
        let running = Arc::new(AtomicBool::new(true));
        let ticks = Arc::new(AtomicU64::new(0));
        let thread_running = Arc::clone(&running);
        let thread_ticks = Arc::clone(&ticks);
        let handle = std::thread::Builder::new()
            .name("mc-plugin-epoch".to_owned())
            .spawn(move || {
                while thread_running.load(Ordering::Acquire) {
                    std::thread::sleep(interval);
                    if !thread_running.load(Ordering::Acquire) {
                        return;
                    }
                    thread_ticks.fetch_add(1, Ordering::Relaxed);
                    engine.increment_epoch();
                }
            })?;
        Ok(Self {
            running,
            ticks,
            handle: Some(handle),
        })
    }

    /// How many epoch ticks this ticker has produced.
    #[must_use]
    pub fn ticks(&self) -> u64 {
        self.ticks.load(Ordering::Relaxed)
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

pub use bindings::solaris::plugin::types::LogLevel;

/// The host-side implementation of the contract's imports.
///
/// Imports are only what a callback may do while it runs: log, and read the id
/// the host bound to the instance. Anything else is a command.
pub trait HostServices: Send {
    /// One bounded diagnostic line from the guest.
    fn log(&mut self, level: LogLevel, message: &str);

    /// The plugin id the host bound to this instance.
    fn plugin_id(&self) -> &str;
}

/// Host-side state of one plugin instance.
pub struct InstanceState<S: HostServices> {
    services: S,
    /// The store-level bounds Wasmtime enforces on every allocation.
    store_limits: StoreLimits,
    /// The bounds this host enforces itself, next to the store's.
    limits: PluginLimits,
    calls: u64,
    /// Diagnostic lines this callback has emitted, and how many it lost to the
    /// per-call bound. Reset at the start of every callback.
    log_lines: u32,
    log_lines_dropped: u32,
    /// Whether the guest is answering a pre-commit hook right now.
    ///
    /// The hook phase is the one phase a guest cannot reach out of: the contract
    /// refuses every import there except reading the instance's own plugin id, so
    /// a hook cannot log, cannot ask the host for anything and cannot stage an
    /// effect while it is deciding one. The phase is the store's own, so it ends
    /// with the call whatever the guest did inside it.
    hook_phase: bool,
    /// Imports the current hook phase refused, reset when the phase begins.
    hook_violations: u32,
}

impl<S: HostServices> InstanceState<S> {
    /// State that enforces `limits` for every allocation the guest makes.
    #[must_use]
    pub fn new(services: S, limits: &PluginLimits) -> Self {
        Self {
            services,
            store_limits: StoreLimitsBuilder::new()
                .memory_size(limits.guest_memory_bytes)
                .table_elements(limits.table_elements)
                .instances(limits.instances)
                .tables(limits.tables)
                .memories(limits.memories)
                .build(),
            limits: *limits,
            calls: 0,
            log_lines: 0,
            log_lines_dropped: 0,
            hook_phase: false,
            hook_violations: 0,
        }
    }

    /// The services this instance was built with.
    pub fn services(&mut self) -> &mut S {
        &mut self.services
    }

    /// How many callbacks the host has invoked on this instance.
    #[must_use]
    pub fn calls(&self) -> u64 {
        self.calls
    }

    pub(crate) fn note_call(&mut self) {
        self.calls += 1;
        self.log_lines = 0;
        self.log_lines_dropped = 0;
    }

    /// Diagnostic lines this callback lost to the per-call bound.
    #[must_use]
    pub const fn log_lines_dropped(&self) -> u32 {
        self.log_lines_dropped
    }

    /// Begin one hook call: the imports the hook phase refuses are refused until
    /// the call ends.
    pub(crate) fn enter_hook_phase(&mut self) {
        self.hook_phase = true;
        self.hook_violations = 0;
    }

    /// End one hook call, answering how many imports it refused.
    pub(crate) fn leave_hook_phase(&mut self) -> u32 {
        self.hook_phase = false;
        std::mem::take(&mut self.hook_violations)
    }

    /// Whether the guest is answering a pre-commit hook right now.
    #[must_use]
    pub const fn in_hook_phase(&self) -> bool {
        self.hook_phase
    }

    /// Whether the guest's current phase allows an import that leaves the
    /// instance, counting the refusal when it does not.
    ///
    /// `plugin-id` reads nothing but the identity the host bound to the instance,
    /// so it is the one import a hook may still call; everything else reaches
    /// outside, and a hook that reaches for one has answered nothing the owner may
    /// commit - which is why the refusal is counted rather than silently ignored.
    pub(crate) fn allow_import(&mut self) -> bool {
        if self.hook_phase {
            self.hook_violations = self.hook_violations.saturating_add(1);
            return false;
        }
        true
    }
}

/// One compiled component, reusable across instances.
pub struct CompiledPlugin {
    component: Component,
    api_version: String,
}

impl CompiledPlugin {
    /// Compile `bytes` as a component of the `solaris:plugin` contract.
    ///
    /// The artifact size bound is checked before compilation, because compiling
    /// a component is host work that a guest's fuel budget does not pay for.
    pub fn compile(
        engine: &Engine,
        bytes: &[u8],
        limits: &PluginLimits,
        api_version: &str,
    ) -> Result<Self, HostError> {
        if bytes.len() > limits.artifact_bytes {
            return Err(HostError::Component(format!(
                "artifact is {} bytes, the bound is {}",
                bytes.len(),
                limits.artifact_bytes
            )));
        }
        let component = Component::new(engine, bytes)
            .map_err(|error| HostError::Component(error.to_string()))?;
        Ok(Self {
            component,
            api_version: api_version.to_owned(),
        })
    }

    /// The contract version this component was compiled as.
    #[must_use]
    pub fn api_version(&self) -> &str {
        &self.api_version
    }

    /// The underlying component.
    #[must_use]
    pub fn component(&self) -> &Component {
        &self.component
    }
}

/// Build the engine every plugin instance of this host shares.
///
/// Compilation is bounded and sequential per engine call; the epoch mechanism is
/// enabled here so a `Store` can set a deadline before it runs guest code.
pub fn engine(limits: &PluginLimits) -> Result<Engine, HostError> {
    let mut config = Config::new();
    config
        .wasm_component_model(true)
        .consume_fuel(true)
        .epoch_interruption(true)
        .wasm_backtrace(false)
        .parallel_compilation(false)
        .max_wasm_stack(usize::try_from(limits.wasm_stack_bytes).unwrap_or(usize::MAX));
    Engine::new(&config).map_err(|error| HostError::Engine(error.to_string()))
}

/// A `Store` prepared for one plugin instance, with every limit already applied.
pub fn store<S: HostServices>(
    engine: &Engine,
    services: S,
    limits: &PluginLimits,
) -> Result<Store<InstanceState<S>>, HostError> {
    let mut store = Store::new(engine, InstanceState::new(services, limits));
    store.limiter(|state| &mut state.store_limits);
    store
        .set_fuel(limits.fuel_per_call)
        .map_err(|error| HostError::Engine(error.to_string()))?;
    store.set_epoch_deadline(limits.epoch_ticks_per_call);
    // The store limiter bounds what the *guest* allocates. What the host
    // allocates while lifting an answer is bounded here instead, and Wasmtime
    // spends this budget before it copies, so a hostile answer is refused rather
    // than copied first and refused afterwards. The budget is per copy context
    // (one callback's answer, one call into an import), which is why it does not
    // accumulate and needs no re-arming between calls.
    store.set_hostcall_fuel(limits.hostcall_bytes);
    Ok(store)
}

/// The linker every instance of this contract is instantiated with.
pub fn linker<T: HostServices + 'static>(
    engine: &Engine,
) -> Result<Linker<crate::InstanceState<T>>, HostError> {
    let mut linker = Linker::new(engine);
    // The store's data *is* the plugin state, so the accessor is the identity and
    // `HasSelf` names that shape for the generated binder.
    bindings::Plugin::add_to_linker::<
        crate::InstanceState<T>,
        wasmtime::component::HasSelf<crate::InstanceState<T>>,
    >(&mut linker, |state: &mut crate::InstanceState<T>| state)
    .map_err(|error| HostError::Engine(error.to_string()))?;
    Ok(linker)
}

/// The generated host side of the `solaris:plugin` contract.
///
/// Every public item here comes from the WIT in `crates/mc-script/wit`; nothing
/// in this crate declares a second copy of the contract's types.
pub mod bindings {
    wasmtime::component::bindgen!({
        path: "../mc-script/wit",
        world: "plugin",
    });
}
