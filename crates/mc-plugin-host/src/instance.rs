//! One instantiated plugin: its store, its callbacks and its budget.
//!
//! Every callback runs under the same discipline: the store's fuel is refilled
//! to the configured allowance, its epoch deadline is re-armed, the guest is
//! called, and whatever it returned is staged before the caller can apply it. A
//! trap, an exhausted budget or an answer the contract refuses leaves the
//! staged batch unapplied and retires the instance - a guest's own memory is not
//! assumed to be rolled back by a trap.

use wasmtime::component::{Component, Linker};
use wasmtime::{Store, Trap};

use crate::bindings::Plugin;
use crate::bindings::exports::solaris::plugin::events::{Event, EventContext};
use crate::bindings::exports::solaris::plugin::lifecycle::{InitContext, RulePlan};
use crate::bindings::solaris::plugin::commands::Command;
use crate::bindings::solaris::plugin::host::Host;
use crate::bindings::solaris::plugin::types::LogLevel;
use crate::bindings::solaris::plugin::types::PluginError;
use crate::{
    CommandBatch, HostError, HostServices, InstanceState, PluginLimits, StagingError,
    TRUNCATION_MARKER,
};

/// One live plugin instance.
pub struct PluginInstance<S: HostServices + 'static> {
    store: Store<InstanceState<S>>,
    plugin: Plugin,
    limits: PluginLimits,
    retired: Option<HostError>,
}

impl<S: HostServices + 'static> PluginInstance<S> {
    /// Instantiate a compiled component with every limit already applied.
    ///
    /// Limits are set on the store *before* instantiation because a component's
    /// initializers are guest code: a package must not be able to allocate or
    /// spin its way past its budget merely by doing it during `start`.
    pub fn instantiate(
        linker: &Linker<InstanceState<S>>,
        component: &Component,
        services: S,
        limits: PluginLimits,
    ) -> Result<Self, HostError> {
        let mut store = crate::store(linker.engine(), services, &limits)?;
        let plugin = match Plugin::instantiate(&mut store, component, linker) {
            Ok(plugin) => plugin,
            Err(error) => {
                // Two different problems fail here, and the operator has to be able
                // to tell them apart: the *host* refused the component before any
                // guest code ran - the linker cannot satisfy its imports, its types
                // do not match this contract, or a limit the store was built with
                // denies what it asks for - or a guest initializer faulted, which
                // is a trap or an exhausted budget. Wasmtime reports the first
                // class without a `Trap`, which is what separates them; the enum's
                // own `Instantiate` variant used to be unreachable because every
                // failure was classified as a guest fault.
                let trapped = error.downcast_ref::<Trap>().is_some();
                return Err(if trapped {
                    classify(store.engine(), "instantiate", error)
                } else {
                    HostError::Instantiate(error.to_string())
                });
            }
        };
        Ok(Self {
            store,
            plugin,
            limits,
            retired: None,
        })
    }

    /// The host state of this instance: its services and its call counter.
    pub fn state(&mut self) -> &mut InstanceState<S> {
        self.store.data_mut()
    }

    /// The store this instance runs in.
    pub fn store(&mut self) -> &mut Store<InstanceState<S>> {
        &mut self.store
    }

    /// Whether this instance was retired by a failed call.
    #[must_use]
    pub fn retired_because(&self) -> Option<&HostError> {
        self.retired.as_ref()
    }

    /// The startup-phase call. It may produce a rule plan and nothing else.
    pub fn configure(&mut self, config: &str) -> Result<Option<RulePlan>, HostError> {
        self.call("configure", |instance| {
            instance
                .plugin
                .solaris_plugin_lifecycle()
                .call_configure(&mut instance.store, config)
        })
        .and_then(|answer| match answer {
            Ok(plan) => Ok(plan),
            Err(error) => Err(HostError::Answer(format!("{error:?}"))),
        })
    }

    /// The runtime-phase call, one per instance. Its commands are staged, not
    /// applied: the caller decides whether they are admissible.
    pub fn init(&mut self, config: &str, context: InitContext) -> Result<CommandBatch, HostError> {
        let answer = self.call("init", |instance| {
            instance.plugin.solaris_plugin_lifecycle().call_init(
                &mut instance.store,
                config,
                &context,
            )
        })?;
        self.stage_answer("init", answer)
    }

    /// One event batch. The returned commands belong to this call only.
    pub fn on_events(
        &mut self,
        context: EventContext,
        events: &[Event],
    ) -> Result<CommandBatch, HostError> {
        if events.len() > self.limits.events_per_batch {
            return Err(HostError::Answer(format!(
                "batch holds {} events, the bound is {}",
                events.len(),
                self.limits.events_per_batch
            )));
        }
        let answer = self.call("on-events", |instance| {
            instance.plugin.solaris_plugin_events().call_on_events(
                &mut instance.store,
                context,
                events,
            )
        })?;
        self.stage_answer("on-events", answer)
    }

    /// The best-effort shutdown call. A failure here is reported, never
    /// performed again, and never blocks the host from reclaiming the instance.
    pub fn shutdown(&mut self) -> Result<(), HostError> {
        let answer = self.call("shutdown", |instance| {
            instance
                .plugin
                .solaris_plugin_lifecycle()
                .call_shutdown(&mut instance.store)
        })?;
        match answer {
            Ok(()) => Ok(()),
            Err(error) => Err(HostError::Answer(format!("{error:?}"))),
        }
    }

    /// Run one guest call with the budget re-armed, and retire the instance on
    /// any failure: a trapped guest is not called again.
    fn call<T>(
        &mut self,
        what: &str,
        invoke: impl FnOnce(&mut Self) -> Result<T, wasmtime::Error>,
    ) -> Result<T, HostError> {
        if let Some(error) = &self.retired {
            return Err(HostError::Answer(format!(
                "instance was retired earlier: {error}"
            )));
        }
        if let Err(error) = self.store.set_fuel(self.limits.fuel_per_call) {
            let failure = HostError::Engine(error.to_string());
            self.retired = Some(HostError::Engine(error.to_string()));
            return Err(failure);
        }
        self.store
            .set_epoch_deadline(self.limits.epoch_ticks_per_call);
        self.store.data_mut().note_call();
        match invoke(self) {
            Ok(value) => Ok(value),
            Err(error) => {
                let failure = classify(self.store.engine(), what, error);
                // The classified failure is what the instance is retired for: a
                // trap must not be reported as an invalid answer.
                self.retired = Some(failure.clone());
                Err(failure)
            }
        }
    }

    fn stage_answer(&mut self, what: &str, answer: GuestAnswer) -> Result<CommandBatch, HostError> {
        let commands = match answer {
            Ok(commands) => commands,
            Err(error) => {
                return Err(HostError::Answer(format!(
                    "{what} refused by the guest: {error:?}"
                )));
            }
        };
        let mut batch = CommandBatch::new();
        if let Err(error) = batch.extend(commands, &self.limits) {
            let failure = match error {
                StagingError::TooManyCommands => HostError::Answer(format!(
                    "{what} returned more commands than the bound of {}",
                    self.limits.commands_per_call
                )),
                StagingError::TextTooLong => HostError::Answer(format!(
                    "{what} returned text past the bound of {} bytes",
                    self.limits.text_bytes
                )),
            };
            // A guest that answers with too much is retired like one that traps:
            // the answer is not applied and the instance is not called again.
            self.retired = Some(failure.clone());
            return Err(failure);
        }
        Ok(batch)
    }
}

/// The answer shape every callback shares: the contract's command list, or the
/// error the guest itself reported.
type GuestAnswer = Result<Vec<Command>, PluginError>;

/// The marker interfaces carry no functions of their own, but the contract's
/// other interfaces use their types, so the generated bindings require them for
/// the store's data.
impl<S: HostServices> crate::bindings::solaris::plugin::types::Host for InstanceState<S> {}

impl<S: HostServices> crate::bindings::solaris::plugin::commands::Host for InstanceState<S> {}

impl<S: HostServices> crate::bindings::solaris::plugin::storage::Host for InstanceState<S> {}

impl<S: HostServices + 'static> Host for InstanceState<S> {
    fn log(&mut self, level: LogLevel, message: String) {
        // The contract promises a size- and rate-limited diagnostic line: the
        // limits are the host's, so a guest cannot choose what the operator's log
        // costs. A line past the per-call count is counted and dropped rather
        // than sent.
        let (line_bytes, lines_per_call) =
            (self.limits.log_line_bytes, self.limits.log_lines_per_call);
        if self.log_lines >= lines_per_call {
            self.log_lines_dropped = self.log_lines_dropped.saturating_add(1);
            return;
        }
        self.log_lines += 1;
        let services = self.services();
        if message.len() <= line_bytes {
            services.log(level, &message);
            return;
        }
        let mut end = line_bytes
            .saturating_sub(TRUNCATION_MARKER.len())
            .min(message.len());
        while end > 0 && !message.is_char_boundary(end) {
            end -= 1;
        }
        services.log(level, &format!("{}{TRUNCATION_MARKER}", &message[..end]));
    }

    fn plugin_id(&mut self) -> String {
        self.services().plugin_id().to_owned()
    }
}

/// Turn a Wasmtime failure into the host's own error, keeping an exhausted
/// budget distinct from a guest that trapped for its own reasons.
fn classify(engine: &wasmtime::Engine, what: &str, error: wasmtime::Error) -> HostError {
    let _ = engine;
    match error.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => HostError::Budget,
        Some(Trap::Interrupt) => HostError::Budget,
        Some(trap) => HostError::Trap(format!("{what}: {trap}")),
        None => HostError::Trap(format!("{what}: {error}")),
    }
}
