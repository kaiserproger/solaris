//! One instantiated plugin: its stores, its callbacks and its budget.
//!
//! A package's phases run in two stores and never in the same one. `configure`
//! answers the startup rules from a store of its own that the host drops before
//! the runtime store exists; `init`, `on-events` and `shutdown` run in the
//! runtime store for the instance's whole life. The split is the type system's:
//! [`PluginStartup`] has only the startup call and [`PluginInstance`] has only the
//! runtime calls, so nothing a guest wrote while answering startup rules can
//! reach a later phase, and no runtime callback can run in the store the startup
//! phase used.
//!
//! Every runtime callback runs under the same discipline: the store's fuel is
//! refilled to the configured allowance, its epoch deadline is re-armed, the guest
//! is called, and whatever it returned is staged before the caller can apply it. A
//! trap, an exhausted budget or an answer the contract refuses leaves the staged
//! batch unapplied and retires the instance - a guest's own memory is not assumed
//! to be rolled back by a trap.
//!
//! One pushed simulation tick is the exception, and it is armed by the caller:
//! the tick's due timer callbacks share the one allowance [`PluginInstance::arm_delivery`]
use std::time::Instant;

use crate::bindings::Plugin;
use crate::bindings::exports::solaris::plugin::events::{Event, EventContext};
use crate::bindings::exports::solaris::plugin::lifecycle::{InitContext, StartupContribution};
use crate::bindings::exports::solaris::plugin::precommit::{
    BuildContext, BuildDecision, DamageContext, DamageDecision, DamageTarget, HookActor, HookPlayer,
};
use crate::bindings::solaris::plugin::commands::Command;
use crate::bindings::solaris::plugin::host::Host;
use crate::bindings::solaris::plugin::types::LogLevel;
use crate::bindings::solaris::plugin::types::PluginError;
use crate::{
    CommandBatch, HostError, HostServices, InstanceState, PluginLimits, StagingError,
    TRUNCATION_MARKER,
};
use mc_script::precommit::{
    BuildContext as NativeBuildContext, DamageContext as NativeDamageContext,
    DamageTarget as NativeDamageTarget, HookActor as NativeHookActor, HookDecision,
};
use wasmtime::component::{Component, Linker};
use wasmtime::{Store, Trap};

/// The store-and-plugin core both phases share.
///
/// Its call machinery is the same whoever calls it - the store's budget is armed
/// before the call, a failure classifies and retires - and it is private so the
/// two public types can be the two authorities: [`PluginStartup`] exposes the
/// startup call alone, [`PluginInstance`] the runtime calls alone.
struct Guest<S: HostServices + 'static> {
    store: Store<InstanceState<S>>,
    plugin: Plugin,
    limits: PluginLimits,
    retired: Option<HostError>,
}

impl<S: HostServices + 'static> Guest<S> {
    /// Instantiate a compiled component with every limit already applied.
    ///
    /// Limits are set on the store *before* instantiation because a component's
    /// initializers are guest code: a package must not be able to allocate or
    /// spin its way past its budget merely by doing it during `start`.
    fn instantiate(
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

    /// Run one guest call with the budget re-armed, and retire the instance on
    /// any failure: a trapped guest is not called again.
    fn call<T>(
        &mut self,
        what: &str,
        invoke: impl FnOnce(&mut Self) -> Result<T, wasmtime::Error>,
    ) -> Result<T, HostError> {
        self.arm()?;
        self.call_armed(what, invoke)
    }

    /// Run a hook within the absolute chain deadline, never granting a handler
    /// more epoch ticks than the time all preceding handlers left it.
    fn call_until<T>(
        &mut self,
        what: &str,
        deadline: Instant,
        invoke: impl FnOnce(&mut Self) -> Result<T, wasmtime::Error>,
    ) -> Result<T, HostError> {
        self.arm_until(deadline)?;
        self.call_armed(what, invoke)
    }

    /// Re-arm the allowance of one callback, leaving a retired instance alone.
    fn arm(&mut self) -> Result<(), HostError> {
        self.ensure_live()?;
        if let Err(error) = self.store.set_fuel(self.limits.fuel_per_call) {
            let failure = HostError::Engine(error.to_string());
            self.retired = Some(failure.clone());
            return Err(failure);
        }
        self.store
            .set_epoch_deadline(self.limits.epoch_ticks_per_call);
        Ok(())
    }

    fn arm_until(&mut self, deadline: Instant) -> Result<(), HostError> {
        self.ensure_live()?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(HostError::Budget);
        }
        if let Err(error) = self.store.set_fuel(self.limits.fuel_per_call) {
            let failure = HostError::Engine(error.to_string());
            self.retired = Some(failure.clone());
            return Err(failure);
        }
        let ticks = u64::try_from(
            (remaining.as_millis().saturating_add(24) / 25)
                .max(1)
                .min(u128::from(self.limits.epoch_ticks_per_call)),
        )
        .unwrap_or(self.limits.epoch_ticks_per_call);
        self.store.set_epoch_deadline(ticks);
        Ok(())
    }

    /// Run one guest call on the allowance already armed for it, and retire the
    /// instance if the call itself failed.
    fn call_armed<T>(
        &mut self,
        what: &str,
        invoke: impl FnOnce(&mut Self) -> Result<T, wasmtime::Error>,
    ) -> Result<T, HostError> {
        self.ensure_live()?;
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

    /// Refuse to call an instance that a failed callback already retired.
    fn ensure_live(&self) -> Result<(), HostError> {
        match &self.retired {
            Some(error) => Err(HostError::Answer(format!(
                "instance was retired earlier: {error}"
            ))),
            None => Ok(()),
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

/// One package's `configure` phase, in a store of its own.
///
/// The store is created for the phase and dropped with this value, before the
/// caller builds the runtime store [`PluginInstance`] owns: the two phases never
/// share a store, so a guest that left state behind while answering startup rules
/// cannot see it from `init`. It is deliberately not a [`PluginInstance`] - the
/// runtime calls are not reachable through it - so the phase that answers startup
/// rules has no runtime authority to gain by accident.
///
/// No `shutdown` runs here. The contract's cleanup call is the runtime instance's,
/// and the host reclaims the startup store's memory the moment this value drops.
pub struct PluginStartup<S: HostServices + 'static> {
    guest: Guest<S>,
}

impl<S: HostServices + 'static> PluginStartup<S> {
    /// Instantiate a compiled component for the startup phase alone.
    pub fn instantiate(
        linker: &Linker<InstanceState<S>>,
        component: &Component,
        services: S,
        limits: PluginLimits,
    ) -> Result<Self, HostError> {
        Ok(Self {
            guest: Guest::instantiate(linker, component, services, limits)?,
        })
    }

    /// The startup-phase call. It may produce a startup contribution and nothing
    /// else.
    pub fn configure(&mut self, config: &str) -> Result<Option<StartupContribution>, HostError> {
        self.guest
            .call("configure", |guest| {
                guest
                    .plugin
                    .solaris_plugin_lifecycle()
                    .call_configure(&mut guest.store, config)
            })
            .and_then(|answer| match answer {
                Ok(contribution) => Ok(contribution),
                Err(error) => Err(HostError::Answer(format!("{error:?}"))),
            })
    }
}

/// One live plugin instance: the runtime store of one package.
pub struct PluginInstance<S: HostServices + 'static> {
    guest: Guest<S>,
}

impl<S: HostServices + 'static> PluginInstance<S> {
    /// Instantiate a compiled component as the runtime instance of one package.
    ///
    /// This is the store `init` runs in and the callbacks of the instance's whole
    /// life run in. It is built *after* the package's `configure` phase, whose own
    /// store is already gone by the time this one exists.
    pub fn instantiate(
        linker: &Linker<InstanceState<S>>,
        component: &Component,
        services: S,
        limits: PluginLimits,
    ) -> Result<Self, HostError> {
        Ok(Self {
            guest: Guest::instantiate(linker, component, services, limits)?,
        })
    }

    /// The host state of this instance: its services and its call counter.
    pub fn state(&mut self) -> &mut InstanceState<S> {
        self.guest.store.data_mut()
    }

    /// The store this instance runs in.
    pub fn store(&mut self) -> &mut Store<InstanceState<S>> {
        &mut self.guest.store
    }

    /// Whether this instance was retired by a failed call.
    #[must_use]
    pub fn retired_because(&self) -> Option<&HostError> {
        self.guest.retired.as_ref()
    }

    /// The runtime-phase call, one per instance. Its commands are staged, not
    /// applied: the caller decides whether they are admissible.
    pub fn init(&mut self, config: &str, context: InitContext) -> Result<CommandBatch, HostError> {
        let answer = self.guest.call("init", |guest| {
            guest
                .plugin
                .solaris_plugin_lifecycle()
                .call_init(&mut guest.store, config, &context)
        })?;
        self.guest.stage_answer("init", answer)
    }

    /// One event batch. The returned commands belong to this call only.
    pub fn on_events(
        &mut self,
        context: EventContext,
        events: &[Event],
    ) -> Result<CommandBatch, HostError> {
        self.arm_delivery()?;
        self.on_events_within_delivery(context, events)
    }

    /// Arm the one allowance a whole pushed-tick delivery shares.
    ///
    /// A guest must not multiply its budget by making many timers due at the same
    /// time: the host arms the delivery once, and every callback of it - up to the
    /// contract's own bound per tick - spends what is left of that same allowance
    /// instead of a fresh one.
    pub(crate) fn arm_delivery(&mut self) -> Result<(), HostError> {
        self.guest.arm()
    }

    /// One callback of a delivery that is already armed.
    ///
    /// It is the same call as [`Self::on_events`] minus the arming, so the caller
    /// that drives a pushed tick's timers spends one allowance on all of them.
    /// Calling it without an armed delivery runs the guest on whatever fuel is
    /// left in its store.
    pub(crate) fn on_events_within_delivery(
        &mut self,
        context: EventContext,
        events: &[Event],
    ) -> Result<CommandBatch, HostError> {
        if events.len() > self.guest.limits.events_per_batch {
            return Err(HostError::Answer(format!(
                "batch holds {} events, the bound is {}",
                events.len(),
                self.guest.limits.events_per_batch
            )));
        }
        let answer = self.guest.call_armed("on-events", |guest| {
            guest
                .plugin
                .solaris_plugin_events()
                .call_on_events(&mut guest.store, context, events)
        })?;
        self.guest.stage_answer("on-events", answer)
    }

    /// Ask the guest to decide a build without opening the event/command phase.
    pub fn before_build(
        &mut self,
        context: &NativeBuildContext,
        deadline: Instant,
    ) -> Result<HookDecision, HostError> {
        let context = build_context(context)?;
        self.hook_call("before-build", deadline, |guest| {
            guest
                .plugin
                .solaris_plugin_precommit()
                .call_before_build(&mut guest.store, &context)
        })
        .and_then(|answer| match answer {
            Ok(BuildDecision::Keep) => Ok(HookDecision::Keep),
            Ok(BuildDecision::Cancel) => Ok(HookDecision::Cancel),
            Err(error) => Err(HostError::Answer(format!(
                "before-build refused by the guest: {error:?}"
            ))),
        })
    }

    /// Ask the guest to decide the raw damage amount currently carried by the
    /// ordered chain.
    pub fn before_damage(
        &mut self,
        context: &NativeDamageContext,
        deadline: Instant,
    ) -> Result<HookDecision, HostError> {
        let context = damage_context(context)?;
        self.hook_call("before-damage", deadline, |guest| {
            guest
                .plugin
                .solaris_plugin_precommit()
                .call_before_damage(&mut guest.store, &context)
        })
        .and_then(|answer| match answer {
            Ok(DamageDecision::Keep) => Ok(HookDecision::Keep),
            Ok(DamageDecision::Cancel) => Ok(HookDecision::Cancel),
            Ok(DamageDecision::Replace(amount)) => {
                HookDecision::replacement(amount).ok_or_else(|| {
                    HostError::Answer("before-damage returned an invalid replacement".to_owned())
                })
            }
            Err(error) => Err(HostError::Answer(format!(
                "before-damage refused by the guest: {error:?}"
            ))),
        })
    }

    fn hook_call<T>(
        &mut self,
        what: &str,
        deadline: Instant,
        invoke: impl FnOnce(&mut Guest<S>) -> Result<T, wasmtime::Error>,
    ) -> Result<T, HostError> {
        self.guest.store.data_mut().enter_hook_phase();
        let answer = self.guest.call_until(what, deadline, invoke);
        let violations = self.guest.store.data_mut().leave_hook_phase();
        if violations == 0 {
            answer
        } else {
            Err(HostError::Answer(format!(
                "{what} attempted {violations} forbidden host import(s)"
            )))
        }
    }

    /// The best-effort shutdown call. A failure here is reported, never
    /// performed again, and never blocks the host from reclaiming the instance.
    pub fn shutdown(&mut self) -> Result<(), HostError> {
        let answer = self.guest.call("shutdown", |guest| {
            guest
                .plugin
                .solaris_plugin_lifecycle()
                .call_shutdown(&mut guest.store)
        })?;
        match answer {
            Ok(()) => Ok(()),
            Err(error) => Err(HostError::Answer(format!("{error:?}"))),
        }
    }
}

fn build_context(context: &NativeBuildContext) -> Result<BuildContext, HostError> {
    Ok(BuildContext {
        actor: hook_actor(context.actor())?,
        dimension: context.dimension().to_owned(),
        edits: context
            .edits()
            .iter()
            .map(
                |edit| crate::bindings::exports::solaris::plugin::precommit::BuildEdit {
                    x: edit.x,
                    y: edit.y,
                    z: edit.z,
                    previous_state: edit.previous_state,
                    proposed_state: edit.proposed_state,
                },
            )
            .collect(),
    })
}

fn damage_context(context: &NativeDamageContext) -> Result<DamageContext, HostError> {
    Ok(DamageContext {
        source: hook_actor(context.source())?,
        target: match context.target() {
            NativeDamageTarget::Player(player) => DamageTarget::Player(HookPlayer {
                uuid: player.uuid().to_owned(),
                session: player.session(),
            }),
            NativeDamageTarget::Entity(entity) => DamageTarget::Entity(*entity),
            _ => {
                return Err(HostError::Answer(
                    "before-damage received an unsupported damage target".to_owned(),
                ));
            }
        },
        kind: context.kind().to_owned(),
        dimension: context.dimension().to_owned(),
        position: crate::bindings::solaris::plugin::types::Position {
            x: context.position().x(),
            y: context.position().y(),
            z: context.position().z(),
        },
        amount: context.amount(),
    })
}

fn hook_actor(actor: &NativeHookActor) -> Result<HookActor, HostError> {
    match actor {
        NativeHookActor::Player(player) => Ok(HookActor::Player(HookPlayer {
            uuid: player.uuid().to_owned(),
            session: player.session(),
        })),
        NativeHookActor::Entity(entity) => Ok(HookActor::Entity(*entity)),
        NativeHookActor::Plugin(plugin) => Ok(HookActor::Plugin(plugin.clone())),
        NativeHookActor::Environment => Ok(HookActor::Environment),
        _ => Err(HostError::Answer(
            "pre-commit hook received an unsupported actor".to_owned(),
        )),
    }
}

/// The answer shape every callback shares: the contract's command list, or the
/// error the guest itself reported.
type GuestAnswer = Result<Vec<Command>, PluginError>;

/// The marker interfaces carry no functions of their own, but the contract's
/// other interfaces use their types, so the generated bindings require them for
/// the store's data.
impl<S: HostServices> crate::bindings::solaris::plugin::types::Host for InstanceState<S> {}

impl<S: HostServices> crate::bindings::solaris::plugin::client_presentation::Host
    for InstanceState<S>
{
}

impl<S: HostServices> crate::bindings::solaris::plugin::commands::Host for InstanceState<S> {}

impl<S: HostServices> crate::bindings::solaris::plugin::storage::Host for InstanceState<S> {}

impl<S: HostServices> crate::bindings::solaris::plugin::settlements::Host for InstanceState<S> {}

impl<S: HostServices> crate::bindings::solaris::plugin::inventories::Host for InstanceState<S> {}

impl<S: HostServices> crate::bindings::solaris::plugin::residents::Host for InstanceState<S> {}

impl<S: HostServices> crate::bindings::solaris::plugin::domain_operations::Host
    for InstanceState<S>
{
}

impl<S: HostServices> crate::bindings::solaris::plugin::operation_types::Host for InstanceState<S> {}

impl<S: HostServices> crate::bindings::solaris::plugin::world_events::Host for InstanceState<S> {}

impl<S: HostServices + 'static> Host for InstanceState<S> {
    fn log(&mut self, level: LogLevel, message: String) {
        if !self.allow_import() {
            return;
        }
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
