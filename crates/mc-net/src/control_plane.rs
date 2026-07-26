//! Bounded runtime-control decisions for chunk throughput and view budgets.
//!
//! This module is deliberately local-process only. It makes per-instance
//! backpressure decisions observable; it does not coordinate shared-world
//! horizontal sharding.

use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio::sync::Notify;

use crate::memory_pressure::{
    MemoryPressureHandle, MemoryPressureObservation, MemoryPressureSampler,
    spawn_memory_pressure_sampler,
};

/// A producer-observed state change that may require runtime admission control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeControlSignal {
    ChunkPressure { saturated_sources: usize },
    FirstChunkSla { active_sources: usize },
    SlowClientShed,
}

#[derive(Debug)]
struct RuntimeControlSignalState {
    chunk_pressure_changed: bool,
    first_chunk_sla_changed: bool,
    slow_client_shed: bool,
    saturated_chunk_sources: usize,
    pending_chunk_saturation_peak: usize,
    active_first_chunk_sla_sources: usize,
    pending_first_chunk_sla_peak: usize,
    receiver_open: bool,
}

impl Default for RuntimeControlSignalState {
    fn default() -> Self {
        Self {
            chunk_pressure_changed: false,
            first_chunk_sla_changed: false,
            slow_client_shed: false,
            saturated_chunk_sources: 0,
            pending_chunk_saturation_peak: 0,
            active_first_chunk_sla_sources: 0,
            pending_first_chunk_sla_peak: 0,
            receiver_open: true,
        }
    }
}

#[derive(Debug, Default)]
struct RuntimeControlSignalChannel {
    state: Mutex<RuntimeControlSignalState>,
    changed: Notify,
}

/// A bounded, non-blocking producer for runtime-control state changes.
#[derive(Debug, Clone)]
pub(crate) struct RuntimeControlSignalProducer {
    channel: Arc<RuntimeControlSignalChannel>,
}

impl RuntimeControlSignalProducer {
    /// Returns false only after the sole consumer has been dropped.
    pub(crate) fn push_slow_client_shed(&self) -> bool {
        let mut state = self
            .channel
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !state.receiver_open {
            return false;
        }
        state.slow_client_shed = true;
        self.channel.changed.notify_one();
        true
    }

    pub(crate) fn chunk_pressure_source(&self) -> RuntimeControlChunkPressureSource {
        RuntimeControlChunkPressureSource {
            channel: Arc::clone(&self.channel),
            saturated: false,
        }
    }

    pub(crate) fn first_chunk_sla_source(&self) -> RuntimeControlFirstChunkSlaSource {
        RuntimeControlFirstChunkSlaSource {
            channel: Arc::clone(&self.channel),
            active: false,
        }
    }
}

#[derive(Debug)]
pub(crate) struct RuntimeControlSignalReceiver {
    channel: Arc<RuntimeControlSignalChannel>,
}

impl RuntimeControlSignalReceiver {
    pub(crate) async fn recv(&mut self) -> Option<RuntimeControlSignal> {
        self.recv_after_registration(std::future::ready(())).await
    }

    async fn recv_after_registration<F>(
        &mut self,
        after_registration: F,
    ) -> Option<RuntimeControlSignal>
    where
        F: Future<Output = ()>,
    {
        let mut after_registration = Some(after_registration);
        loop {
            let changed = self.channel.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(after_registration) = after_registration.take() {
                after_registration.await;
            }
            if let Some(signal) = self.take_pending() {
                return Some(signal);
            }
            changed.await;
        }
    }

    fn take_pending(&self) -> Option<RuntimeControlSignal> {
        let mut state = self
            .channel
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.slow_client_shed {
            state.slow_client_shed = false;
            return Some(RuntimeControlSignal::SlowClientShed);
        }
        if state.first_chunk_sla_changed {
            let active_sources = state
                .pending_first_chunk_sla_peak
                .max(state.active_first_chunk_sla_sources);
            if active_sources > state.active_first_chunk_sla_sources {
                state.pending_first_chunk_sla_peak = state.active_first_chunk_sla_sources;
            } else {
                state.first_chunk_sla_changed = false;
            }
            return Some(RuntimeControlSignal::FirstChunkSla { active_sources });
        }
        if state.chunk_pressure_changed {
            let saturated_sources = state
                .pending_chunk_saturation_peak
                .max(state.saturated_chunk_sources);
            if saturated_sources > state.saturated_chunk_sources {
                state.pending_chunk_saturation_peak = state.saturated_chunk_sources;
            } else {
                state.chunk_pressure_changed = false;
            }
            return Some(RuntimeControlSignal::ChunkPressure { saturated_sources });
        }
        None
    }

    #[cfg(test)]
    pub(crate) fn try_recv(&mut self) -> Option<RuntimeControlSignal> {
        self.take_pending()
    }
}

impl Drop for RuntimeControlSignalReceiver {
    fn drop(&mut self) {
        let mut state = self
            .channel
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.receiver_open = false;
        self.channel.changed.notify_waiters();
    }
}

#[derive(Debug)]
pub(crate) struct RuntimeControlChunkPressureSource {
    channel: Arc<RuntimeControlSignalChannel>,
    saturated: bool,
}

impl RuntimeControlChunkPressureSource {
    pub(crate) fn set_saturated(&mut self, saturated: bool) -> bool {
        if saturated == self.saturated {
            let state = self
                .channel
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            return state.receiver_open;
        }

        self.saturated = saturated;
        let mut state = self
            .channel
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let had_pending_change = state.chunk_pressure_changed;
        if saturated {
            state.saturated_chunk_sources = state
                .saturated_chunk_sources
                .checked_add(1)
                .expect("active chunk pressure source count overflowed");
        } else {
            state.saturated_chunk_sources = state
                .saturated_chunk_sources
                .checked_sub(1)
                .expect("chunk pressure source recovered without matching saturation");
        }
        if had_pending_change {
            state.pending_chunk_saturation_peak = state
                .pending_chunk_saturation_peak
                .max(state.saturated_chunk_sources);
        } else {
            state.pending_chunk_saturation_peak = state.saturated_chunk_sources;
        }
        if !state.receiver_open {
            return false;
        }
        state.chunk_pressure_changed = true;
        self.channel.changed.notify_one();
        true
    }
}

impl Drop for RuntimeControlChunkPressureSource {
    fn drop(&mut self) {
        if self.saturated {
            let _ = self.set_saturated(false);
        }
    }
}

#[derive(Debug)]
pub(crate) struct RuntimeControlFirstChunkSlaSource {
    channel: Arc<RuntimeControlSignalChannel>,
    active: bool,
}

impl RuntimeControlFirstChunkSlaSource {
    pub(crate) fn set_active(&mut self, active: bool) -> bool {
        if active == self.active {
            let state = self
                .channel
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            return state.receiver_open;
        }

        self.active = active;
        let mut state = self
            .channel
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let had_pending_change = state.first_chunk_sla_changed;
        if active {
            state.active_first_chunk_sla_sources = state
                .active_first_chunk_sla_sources
                .checked_add(1)
                .expect("active first-chunk SLA source count overflowed");
        } else {
            state.active_first_chunk_sla_sources = state
                .active_first_chunk_sla_sources
                .checked_sub(1)
                .expect("first-chunk SLA source recovered without matching pressure");
        }
        if had_pending_change {
            state.pending_first_chunk_sla_peak = state
                .pending_first_chunk_sla_peak
                .max(state.active_first_chunk_sla_sources);
        } else {
            state.pending_first_chunk_sla_peak = state.active_first_chunk_sla_sources;
        }
        if !state.receiver_open {
            return false;
        }
        state.first_chunk_sla_changed = true;
        self.channel.changed.notify_one();
        true
    }
}

impl Drop for RuntimeControlFirstChunkSlaSource {
    fn drop(&mut self) {
        if self.active {
            let _ = self.set_active(false);
        }
    }
}

fn runtime_control_signal_channel() -> (RuntimeControlSignalProducer, RuntimeControlSignalReceiver)
{
    let channel = Arc::new(RuntimeControlSignalChannel::default());
    (
        RuntimeControlSignalProducer {
            channel: Arc::clone(&channel),
        },
        RuntimeControlSignalReceiver { channel },
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoscaleProfile {
    LowEnd,
    Balanced,
    HighEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoscalePolicy {
    pub profile: AutoscaleProfile,
    pub min_view_distance: i32,
    pub max_view_distance: i32,
    pub min_chunk_send_rate: u32,
    pub max_chunk_send_rate: u32,
    pub min_chunk_load_rate: u32,
    pub max_chunk_load_rate: u32,
    pub min_chunk_generate_rate: u32,
    pub max_chunk_generate_rate: u32,
    pub target_tick_ms: u64,
    pub target_first_chunk_ms: u64,
    pub queue_pressure_percent: u8,
    pub memory_pressure_percent: u8,
    pub scale_down_after_ticks: u32,
    pub scale_up_after_ticks: u32,
}

impl Default for AutoscalePolicy {
    fn default() -> Self {
        Self::for_profile(AutoscaleProfile::Balanced)
    }
}

impl AutoscalePolicy {
    #[must_use]
    pub fn for_profile(profile: AutoscaleProfile) -> Self {
        match profile {
            AutoscaleProfile::LowEnd => Self {
                profile,
                min_view_distance: 4,
                max_view_distance: 8,
                min_chunk_send_rate: 4,
                max_chunk_send_rate: 8,
                min_chunk_load_rate: 8,
                max_chunk_load_rate: 16,
                min_chunk_generate_rate: 4,
                max_chunk_generate_rate: 12,
                target_tick_ms: 50,
                target_first_chunk_ms: 2_500,
                queue_pressure_percent: 70,
                memory_pressure_percent: 85,
                scale_down_after_ticks: 2,
                scale_up_after_ticks: 8,
            },
            AutoscaleProfile::Balanced => Self {
                profile,
                min_view_distance: 6,
                max_view_distance: 10,
                min_chunk_send_rate: 8,
                max_chunk_send_rate: 16,
                min_chunk_load_rate: 16,
                max_chunk_load_rate: 64,
                min_chunk_generate_rate: 8,
                max_chunk_generate_rate: 32,
                target_tick_ms: 50,
                target_first_chunk_ms: 1_500,
                queue_pressure_percent: 75,
                memory_pressure_percent: 85,
                scale_down_after_ticks: 3,
                scale_up_after_ticks: 10,
            },
            AutoscaleProfile::HighEnd => Self {
                profile,
                min_view_distance: 8,
                max_view_distance: 32,
                min_chunk_send_rate: 16,
                max_chunk_send_rate: 32,
                min_chunk_load_rate: 32,
                max_chunk_load_rate: 96,
                min_chunk_generate_rate: 16,
                max_chunk_generate_rate: 64,
                target_tick_ms: 50,
                target_first_chunk_ms: 1_000,
                queue_pressure_percent: 80,
                memory_pressure_percent: 90,
                scale_down_after_ticks: 4,
                scale_up_after_ticks: 12,
            },
        }
    }

    #[must_use]
    pub fn normalized(self) -> Self {
        let min_view_distance = self
            .min_view_distance
            .clamp(crate::MIN_VIEW_DISTANCE, crate::MAX_VIEW_DISTANCE);
        Self {
            profile: self.profile,
            min_view_distance,
            max_view_distance: self
                .max_view_distance
                .clamp(min_view_distance, crate::MAX_VIEW_DISTANCE),
            min_chunk_send_rate: self.min_chunk_send_rate.max(1),
            max_chunk_send_rate: self
                .max_chunk_send_rate
                .max(self.min_chunk_send_rate.max(1)),
            min_chunk_load_rate: self.min_chunk_load_rate.max(1),
            max_chunk_load_rate: self
                .max_chunk_load_rate
                .max(self.min_chunk_load_rate.max(1)),
            min_chunk_generate_rate: self.min_chunk_generate_rate.max(1),
            max_chunk_generate_rate: self
                .max_chunk_generate_rate
                .max(self.min_chunk_generate_rate.max(1)),
            target_tick_ms: self.target_tick_ms.max(1),
            target_first_chunk_ms: self.target_first_chunk_ms.max(1),
            queue_pressure_percent: self.queue_pressure_percent.clamp(1, 100),
            memory_pressure_percent: self.memory_pressure_percent.clamp(1, 100),
            scale_down_after_ticks: self.scale_down_after_ticks.max(1),
            scale_up_after_ticks: self.scale_up_after_ticks.max(1),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeControlLimits {
    pub view_distance: i32,
    pub chunk_send_rate: u32,
    pub chunk_load_rate: u32,
    pub chunk_generate_rate: u32,
}

impl RuntimeControlLimits {
    #[must_use]
    pub fn bounded(self, policy: AutoscalePolicy) -> Self {
        let policy = policy.normalized();
        Self {
            view_distance: self
                .view_distance
                .clamp(policy.min_view_distance, policy.max_view_distance),
            chunk_send_rate: self
                .chunk_send_rate
                .clamp(policy.min_chunk_send_rate, policy.max_chunk_send_rate),
            chunk_load_rate: self
                .chunk_load_rate
                .clamp(policy.min_chunk_load_rate, policy.max_chunk_load_rate),
            chunk_generate_rate: self.chunk_generate_rate.clamp(
                policy.min_chunk_generate_rate,
                policy.max_chunk_generate_rate,
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeWorkBudgets {
    pub entity_pathing_candidates: usize,
    pub random_tick_chunks: usize,
    pub scheduled_ticks: usize,
}

impl Default for RuntimeWorkBudgets {
    fn default() -> Self {
        Self {
            entity_pathing_candidates: 8,
            random_tick_chunks: 64,
            scheduled_ticks: 256,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeWorkInput {
    pub tick_p95_us: u64,
    pub entity_goals_p95_us: u64,
    pub entity_physics_p95_us: u64,
    pub entity_dispatch_p95_us: u64,
    pub random_tick_p95_us: u64,
    pub block_tick_p95_us: u64,
    pub fluid_tick_p95_us: u64,
    pub scheduled_budget_exhausted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeWorkFocus {
    EntitySimulation,
    RandomTicks,
    ScheduledTicks,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeWorkDecision {
    pub action: AutoscaleAction,
    pub focus: Option<RuntimeWorkFocus>,
    pub budgets: RuntimeWorkBudgets,
    pub reason: String,
}

#[derive(Debug, Clone, Copy)]
struct RuntimeWorkBudgetBounds {
    min: RuntimeWorkBudgets,
    max: RuntimeWorkBudgets,
}

impl RuntimeWorkBudgetBounds {
    fn for_profile(profile: AutoscaleProfile) -> Self {
        match profile {
            AutoscaleProfile::LowEnd => Self {
                min: RuntimeWorkBudgets {
                    entity_pathing_candidates: 2,
                    random_tick_chunks: 4,
                    scheduled_ticks: 32,
                },
                max: RuntimeWorkBudgets {
                    entity_pathing_candidates: 4,
                    random_tick_chunks: 32,
                    scheduled_ticks: 128,
                },
            },
            AutoscaleProfile::Balanced => Self {
                min: RuntimeWorkBudgets {
                    entity_pathing_candidates: 4,
                    random_tick_chunks: 8,
                    scheduled_ticks: 64,
                },
                max: RuntimeWorkBudgets {
                    entity_pathing_candidates: 8,
                    random_tick_chunks: 64,
                    scheduled_ticks: 256,
                },
            },
            AutoscaleProfile::HighEnd => Self {
                min: RuntimeWorkBudgets {
                    entity_pathing_candidates: 4,
                    random_tick_chunks: 16,
                    scheduled_ticks: 128,
                },
                max: RuntimeWorkBudgets {
                    entity_pathing_candidates: 8,
                    random_tick_chunks: 128,
                    scheduled_ticks: 1_024,
                },
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeControlInput {
    pub tick_ms: u64,
    pub memory_used_mb: u64,
    pub memory_limit_mb: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoscalePressure {
    TickTime,
    ChunkQueue,
    SlowClientShed,
    Memory,
    FirstChunkSla,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoscaleAction {
    Hold,
    ScaleDown,
    ScaleUp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoscaleDecision {
    pub action: AutoscaleAction,
    pub pressure: Option<AutoscalePressure>,
    pub limits: RuntimeControlLimits,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeControlSnapshot {
    pub policy: AutoscalePolicy,
    pub limits: RuntimeControlLimits,
    pub last_decision: AutoscaleDecision,
    pub work_budgets: RuntimeWorkBudgets,
    pub last_work_decision: RuntimeWorkDecision,
    pub pressure_ticks: u32,
    pub healthy_ticks: u32,
    pub draining: bool,
    pub application_stop_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeControlConfig {
    pub policy: AutoscalePolicy,
    pub initial_limits: RuntimeControlLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RuntimeControlOperation {
    Observe(RuntimeControlInput),
    ObserveSignal(RuntimeControlSignal),
    ObserveWork(RuntimeWorkInput),
    RequestDrain,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeControlOutcome {
    Autoscale(AutoscaleDecision),
    Work(RuntimeWorkDecision),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeControlApplyError {
    /// The applicator may have changed resources and the runtime must stop.
    ControlledStop { reason: String },
}

impl RuntimeControlApplyError {
    pub(crate) fn controlled_stop(reason: impl Into<String>) -> Self {
        Self::ControlledStop {
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for RuntimeControlApplyError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ControlledStop { reason } => {
                write!(
                    formatter,
                    "runtime control requires a controlled stop: {reason}"
                )
            }
        }
    }
}

impl std::error::Error for RuntimeControlApplyError {}

/// Shared runtime-control state.
///
/// Decision-only mutation is intentionally not part of the public API:
///
/// ```compile_fail
/// use mc_net::{RuntimeControlHandle, RuntimeControlInput};
/// fn bypass_application(control: &RuntimeControlHandle, input: RuntimeControlInput) {
///     control.observe(input);
/// }
/// ```
///
/// ```compile_fail
/// use mc_net::RuntimeControlHandle;
/// fn bypass_application(control: &RuntimeControlHandle) {
///     control.request_drain();
/// }
/// ```
///
/// ```compile_fail
/// use mc_net::{RuntimeControlHandle, RuntimeWorkInput};
/// fn bypass_application(control: &RuntimeControlHandle, input: RuntimeWorkInput) {
///     control.observe_work(input);
/// }
/// ```
#[derive(Debug, Clone)]
pub struct RuntimeControlHandle {
    controller: Arc<Mutex<RuntimeControlPlane>>,
    memory_pressure: MemoryPressureHandle,
    signal_producer: RuntimeControlSignalProducer,
    signal_receiver: Arc<Mutex<Option<RuntimeControlSignalReceiver>>>,
}

#[derive(Debug, Clone)]
pub struct RuntimeControlPlane {
    policy: AutoscalePolicy,
    limits: RuntimeControlLimits,
    last_decision: AutoscaleDecision,
    work_bounds: RuntimeWorkBudgetBounds,
    work_budgets: RuntimeWorkBudgets,
    last_work_decision: RuntimeWorkDecision,
    pressure_ticks: u32,
    healthy_ticks: u32,
    active_chunk_saturations: usize,
    active_first_chunk_sla_sources: usize,
    draining: bool,
    application_stop_reason: Option<String>,
}

const SCALE_UP_TICK_HEADROOM_PERCENT: u8 = 80;

impl RuntimeControlPlane {
    #[must_use]
    pub fn new(policy: AutoscalePolicy, initial_limits: RuntimeControlLimits) -> Self {
        let policy = policy.normalized();
        let limits = initial_limits.bounded(policy);
        let work_bounds = RuntimeWorkBudgetBounds::for_profile(policy.profile);
        let work_budgets = work_bounds.max;
        Self {
            policy,
            limits,
            last_decision: AutoscaleDecision {
                action: AutoscaleAction::Hold,
                pressure: None,
                limits,
                reason: "initialized within bounded profile limits".to_string(),
            },
            work_bounds,
            work_budgets,
            last_work_decision: RuntimeWorkDecision {
                action: AutoscaleAction::Hold,
                focus: None,
                budgets: work_budgets,
                reason: "initialized from autoscale profile".to_string(),
            },
            pressure_ticks: 0,
            healthy_ticks: 0,
            active_chunk_saturations: 0,
            active_first_chunk_sla_sources: 0,
            draining: false,
            application_stop_reason: None,
        }
    }

    fn decide_drain(&mut self) -> AutoscaleDecision {
        self.draining = true;
        self.limits = RuntimeControlLimits {
            view_distance: self.policy.min_view_distance,
            chunk_send_rate: self.policy.min_chunk_send_rate,
            chunk_load_rate: self.policy.min_chunk_load_rate,
            chunk_generate_rate: self.policy.min_chunk_generate_rate,
        };
        self.work_budgets = self.work_bounds.min;
        self.last_work_decision = RuntimeWorkDecision {
            action: AutoscaleAction::ScaleDown,
            focus: None,
            budgets: self.work_budgets,
            reason: "drain requested; clamped deferred work to profile minimum".to_string(),
        };
        self.record(AutoscaleDecision {
            action: AutoscaleAction::ScaleDown,
            pressure: None,
            limits: self.limits,
            reason: "drain requested; clamped to minimum chunk throughput".to_string(),
        })
    }

    fn decide_observation(&mut self, input: RuntimeControlInput) -> AutoscaleDecision {
        match self.pressure(input) {
            Some(pressure) => self.observe_pressure(pressure),
            None if percent_at_most_u64(
                input.tick_ms,
                self.policy.target_tick_ms,
                SCALE_UP_TICK_HEADROOM_PERCENT,
            ) =>
            {
                self.observe_healthy()
            }
            None => self.observe_tick_deadband(),
        }
    }

    fn decide_signal(&mut self, signal: RuntimeControlSignal) -> AutoscaleDecision {
        match signal {
            RuntimeControlSignal::ChunkPressure { saturated_sources } => {
                self.active_chunk_saturations = saturated_sources;
                self.observe_source_pressure()
            }
            RuntimeControlSignal::FirstChunkSla { active_sources } => {
                self.active_first_chunk_sla_sources = active_sources;
                self.observe_source_pressure()
            }
            RuntimeControlSignal::SlowClientShed => {
                self.observe_pressure(AutoscalePressure::SlowClientShed)
            }
        }
    }

    fn observe_source_pressure(&mut self) -> AutoscaleDecision {
        if self.active_first_chunk_sla_sources > 0 {
            self.observe_pressure(AutoscalePressure::FirstChunkSla)
        } else if self.active_chunk_saturations > 0 {
            self.observe_pressure(AutoscalePressure::ChunkQueue)
        } else {
            self.observe_recovered_signal()
        }
    }

    fn observe_pressure(&mut self, pressure: AutoscalePressure) -> AutoscaleDecision {
        if self.draining {
            return self.hold_drain();
        }

        self.yield_random_tick_work(pressure);
        self.pressure_ticks = self.pressure_ticks.saturating_add(1);
        self.healthy_ticks = 0;
        if self.pressure_ticks >= self.policy.scale_down_after_ticks {
            let before = self.limits;
            self.limits = self.scale_down();
            let pressure_ticks = self.pressure_ticks;
            self.pressure_ticks = 0;
            return self.record(AutoscaleDecision {
                action: if self.limits == before {
                    AutoscaleAction::Hold
                } else {
                    AutoscaleAction::ScaleDown
                },
                pressure: Some(pressure),
                limits: self.limits,
                reason: format!(
                    "pressure persisted for {} observations; applying bounded degradation",
                    pressure_ticks
                ),
            });
        }
        self.record(AutoscaleDecision {
            action: AutoscaleAction::Hold,
            pressure: Some(pressure),
            limits: self.limits,
            reason: format!(
                "pressure observed for {} observations; waiting for hysteresis",
                self.pressure_ticks
            ),
        })
    }

    fn observe_healthy(&mut self) -> AutoscaleDecision {
        if self.draining {
            return self.hold_drain();
        }

        self.healthy_ticks = self.healthy_ticks.saturating_add(1);
        self.pressure_ticks = 0;
        if self.healthy_ticks >= self.policy.scale_up_after_ticks {
            let before = self.limits;
            self.limits = self.scale_up();
            let healthy_ticks = self.healthy_ticks;
            self.healthy_ticks = 0;
            return self.record(AutoscaleDecision {
                action: if self.limits == before {
                    AutoscaleAction::Hold
                } else {
                    AutoscaleAction::ScaleUp
                },
                pressure: None,
                limits: self.limits,
                reason: format!(
                    "healthy for {} observations; restoring bounded throughput",
                    healthy_ticks
                ),
            });
        }

        self.record(AutoscaleDecision {
            action: AutoscaleAction::Hold,
            pressure: None,
            limits: self.limits,
            reason: format!(
                "healthy for {} observations; waiting for hysteresis",
                self.healthy_ticks
            ),
        })
    }

    fn observe_recovered_signal(&mut self) -> AutoscaleDecision {
        self.pressure_ticks = 0;
        self.record(AutoscaleDecision {
            action: AutoscaleAction::Hold,
            pressure: None,
            limits: self.limits,
            reason: "producer recovered; tick health retains recovery hysteresis".to_string(),
        })
    }

    fn observe_tick_deadband(&mut self) -> AutoscaleDecision {
        if self.draining {
            return self.hold_drain();
        }
        self.pressure_ticks = 0;
        self.healthy_ticks = 0;
        self.record(AutoscaleDecision {
            action: AutoscaleAction::Hold,
            pressure: None,
            limits: self.limits,
            reason: format!(
                "tick is within the recovery deadband; scale-up requires at least {}% headroom",
                100 - u16::from(SCALE_UP_TICK_HEADROOM_PERCENT)
            ),
        })
    }

    fn hold_drain(&mut self) -> AutoscaleDecision {
        self.record(AutoscaleDecision {
            action: AutoscaleAction::Hold,
            pressure: None,
            limits: self.limits,
            reason: "drain active; holding minimum limits".to_string(),
        })
    }

    fn decide_work(&mut self, input: RuntimeWorkInput) -> RuntimeWorkDecision {
        if self.draining {
            return self.record_work(RuntimeWorkDecision {
                action: AutoscaleAction::Hold,
                focus: None,
                budgets: self.work_budgets,
                reason: "drain active; holding minimum deferred-work budgets".to_string(),
            });
        }

        let before = self.work_budgets;
        let target_tick_us = self.policy.target_tick_ms.saturating_mul(1_000);
        let scheduled_us = input
            .block_tick_p95_us
            .saturating_add(input.fluid_tick_p95_us);
        let entity_us = input
            .entity_goals_p95_us
            .saturating_add(input.entity_physics_p95_us)
            .saturating_add(input.entity_dispatch_p95_us);

        let (focus, reason) = if input.scheduled_budget_exhausted {
            self.work_budgets.random_tick_chunks = halve_floor_usize(
                self.work_budgets.random_tick_chunks,
                self.work_bounds.min.random_tick_chunks,
            );
            self.work_budgets.scheduled_ticks = double_ceiling_usize(
                self.work_budgets.scheduled_ticks,
                self.work_bounds.max.scheduled_ticks,
            );
            (
                Some(RuntimeWorkFocus::ScheduledTicks),
                "scheduled work exhausted its budget; preserving its quota and reducing random ticks",
            )
        } else if input.tick_p95_us > target_tick_us
            && entity_us >= input.random_tick_p95_us
            && entity_us >= scheduled_us
        {
            self.work_budgets.entity_pathing_candidates = halve_floor_usize(
                self.work_budgets.entity_pathing_candidates,
                self.work_bounds.min.entity_pathing_candidates,
            );
            (
                Some(RuntimeWorkFocus::EntitySimulation),
                "tick p95 exceeded target; reducing entity pathing search while retaining physics correctness",
            )
        } else if input.tick_p95_us > target_tick_us && input.random_tick_p95_us >= scheduled_us {
            self.work_budgets.random_tick_chunks = halve_floor_usize(
                self.work_budgets.random_tick_chunks,
                self.work_bounds.min.random_tick_chunks,
            );
            (
                Some(RuntimeWorkFocus::RandomTicks),
                "tick p95 exceeded target; reducing the more expensive random-tick class",
            )
        } else if input.tick_p95_us > target_tick_us {
            self.work_budgets.scheduled_ticks = halve_floor_usize(
                self.work_budgets.scheduled_ticks,
                self.work_bounds.min.scheduled_ticks,
            );
            (
                Some(RuntimeWorkFocus::ScheduledTicks),
                "tick p95 exceeded target; reducing the more expensive scheduled-tick class",
            )
        } else {
            self.work_budgets.entity_pathing_candidates = recover_toward_ceiling_usize(
                self.work_budgets.entity_pathing_candidates,
                self.work_bounds.max.entity_pathing_candidates,
            );
            self.work_budgets.random_tick_chunks = recover_toward_ceiling_usize(
                self.work_budgets.random_tick_chunks,
                self.work_bounds.max.random_tick_chunks,
            );
            self.work_budgets.scheduled_ticks = recover_toward_ceiling_usize(
                self.work_budgets.scheduled_ticks,
                self.work_bounds.max.scheduled_ticks,
            );
            (None, "tick p95 is healthy; restoring profile work budgets")
        };

        let action = if self.work_budgets == before {
            AutoscaleAction::Hold
        } else if self.work_budgets.entity_pathing_candidates < before.entity_pathing_candidates
            || self.work_budgets.random_tick_chunks < before.random_tick_chunks
            || self.work_budgets.scheduled_ticks < before.scheduled_ticks
        {
            AutoscaleAction::ScaleDown
        } else {
            AutoscaleAction::ScaleUp
        };
        self.record_work(RuntimeWorkDecision {
            action,
            focus,
            budgets: self.work_budgets,
            reason: reason.to_string(),
        })
    }

    fn yield_random_tick_work(&mut self, pressure: AutoscalePressure) {
        let before = self.work_budgets.random_tick_chunks;
        self.work_budgets.random_tick_chunks =
            halve_floor_usize(before, self.work_bounds.min.random_tick_chunks);
        if self.work_budgets.random_tick_chunks == before {
            return;
        }
        self.last_work_decision = RuntimeWorkDecision {
            action: AutoscaleAction::ScaleDown,
            focus: Some(RuntimeWorkFocus::RandomTicks),
            budgets: self.work_budgets,
            reason: format!(
                "runtime {pressure:?} pressure; random ticks yielded before throughput hysteresis"
            ),
        };
    }

    #[must_use]
    pub fn snapshot(&self) -> RuntimeControlSnapshot {
        RuntimeControlSnapshot {
            policy: self.policy,
            limits: self.limits,
            last_decision: self.last_decision.clone(),
            work_budgets: self.work_budgets,
            last_work_decision: self.last_work_decision.clone(),
            pressure_ticks: self.pressure_ticks,
            healthy_ticks: self.healthy_ticks,
            draining: self.draining,
            application_stop_reason: self.application_stop_reason.clone(),
        }
    }

    fn record(&mut self, decision: AutoscaleDecision) -> AutoscaleDecision {
        self.last_decision = decision.clone();
        decision
    }

    fn record_work(&mut self, decision: RuntimeWorkDecision) -> RuntimeWorkDecision {
        self.last_work_decision = decision.clone();
        decision
    }

    fn pressure(&self, input: RuntimeControlInput) -> Option<AutoscalePressure> {
        if percent_at_least_u64(
            input.memory_used_mb,
            input.memory_limit_mb,
            self.policy.memory_pressure_percent,
        ) {
            return Some(AutoscalePressure::Memory);
        }
        if input.tick_ms > self.policy.target_tick_ms {
            return Some(AutoscalePressure::TickTime);
        }
        if self.active_first_chunk_sla_sources > 0 {
            return Some(AutoscalePressure::FirstChunkSla);
        }
        if self.active_chunk_saturations > 0 {
            return Some(AutoscalePressure::ChunkQueue);
        }
        None
    }

    fn scale_down(&self) -> RuntimeControlLimits {
        RuntimeControlLimits {
            view_distance: (self.limits.view_distance - 1).max(self.policy.min_view_distance),
            chunk_send_rate: halve_floor(
                self.limits.chunk_send_rate,
                self.policy.min_chunk_send_rate,
            ),
            chunk_load_rate: halve_floor(
                self.limits.chunk_load_rate,
                self.policy.min_chunk_load_rate,
            ),
            chunk_generate_rate: halve_floor(
                self.limits.chunk_generate_rate,
                self.policy.min_chunk_generate_rate,
            ),
        }
    }

    fn scale_up(&self) -> RuntimeControlLimits {
        RuntimeControlLimits {
            view_distance: (self.limits.view_distance + 1).min(self.policy.max_view_distance),
            chunk_send_rate: double_ceiling(
                self.limits.chunk_send_rate,
                self.policy.max_chunk_send_rate,
            ),
            chunk_load_rate: double_ceiling(
                self.limits.chunk_load_rate,
                self.policy.max_chunk_load_rate,
            ),
            chunk_generate_rate: double_ceiling(
                self.limits.chunk_generate_rate,
                self.policy.max_chunk_generate_rate,
            ),
        }
    }

    #[cfg(test)]
    pub(crate) fn request_drain(&mut self) -> AutoscaleDecision {
        self.decide_drain()
    }

    #[cfg(test)]
    pub(crate) fn observe(&mut self, input: RuntimeControlInput) -> AutoscaleDecision {
        self.decide_observation(input)
    }

    #[cfg(test)]
    pub(crate) fn observe_signal(&mut self, signal: RuntimeControlSignal) -> AutoscaleDecision {
        self.decide_signal(signal)
    }

    #[cfg(test)]
    pub(crate) fn observe_work(&mut self, input: RuntimeWorkInput) -> RuntimeWorkDecision {
        self.decide_work(input)
    }
}

impl RuntimeControlHandle {
    #[must_use]
    pub fn new(config: RuntimeControlConfig) -> Self {
        Self::build_with_memory_pressure(config, MemoryPressureHandle::default())
    }

    fn build_with_memory_pressure(
        config: RuntimeControlConfig,
        memory_pressure: MemoryPressureHandle,
    ) -> Self {
        let (signal_producer, signal_receiver) = runtime_control_signal_channel();
        Self {
            controller: Arc::new(Mutex::new(RuntimeControlPlane::new(
                config.policy,
                config.initial_limits,
            ))),
            memory_pressure,
            signal_producer,
            signal_receiver: Arc::new(Mutex::new(Some(signal_receiver))),
        }
    }

    #[cfg(test)]
    pub(crate) fn request_drain(&self) -> AutoscaleDecision {
        match self
            .apply(RuntimeControlOperation::RequestDrain, |_, _| Ok(()))
            .expect("test-only drain applicator is infallible")
        {
            RuntimeControlOutcome::Autoscale(decision) => decision,
            RuntimeControlOutcome::Work(_) => unreachable!("drain returns autoscale outcome"),
        }
    }

    #[cfg(test)]
    pub(crate) fn observe(&self, input: RuntimeControlInput) -> AutoscaleDecision {
        match self
            .apply(RuntimeControlOperation::Observe(input), |_, _| Ok(()))
            .expect("test-only observation applicator is infallible")
        {
            RuntimeControlOutcome::Autoscale(decision) => decision,
            RuntimeControlOutcome::Work(_) => {
                unreachable!("tick observation returns autoscale outcome")
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn observe_signal(&self, signal: RuntimeControlSignal) -> AutoscaleDecision {
        match self
            .apply(
                RuntimeControlOperation::ObserveSignal(signal),
                |_, _| Ok(()),
            )
            .expect("test-only signal applicator is infallible")
        {
            RuntimeControlOutcome::Autoscale(decision) => decision,
            RuntimeControlOutcome::Work(_) => {
                unreachable!("signal observation returns autoscale outcome")
            }
        }
    }

    pub(crate) fn push_slow_client_shed(&self) -> bool {
        self.signal_producer.push_slow_client_shed()
    }

    pub(crate) fn chunk_pressure_source(&self) -> RuntimeControlChunkPressureSource {
        self.signal_producer.chunk_pressure_source()
    }

    pub(crate) fn first_chunk_sla_source(&self) -> RuntimeControlFirstChunkSlaSource {
        self.signal_producer.first_chunk_sla_source()
    }

    pub(crate) fn take_signal_receiver(&self) -> Option<RuntimeControlSignalReceiver> {
        self.signal_receiver
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    #[must_use]
    pub fn snapshot(&self) -> RuntimeControlSnapshot {
        self.with_controller(|controller| controller.snapshot())
    }

    pub(crate) fn spawn_memory_pressure_sampler(
        &self,
    ) -> (MemoryPressureSampler, tokio::task::JoinHandle<()>) {
        spawn_memory_pressure_sampler(self.memory_pressure.clone())
    }

    #[cfg(test)]
    pub(crate) fn new_with_memory_pressure(
        config: RuntimeControlConfig,
        memory_pressure: MemoryPressureHandle,
    ) -> Self {
        Self::build_with_memory_pressure(config, memory_pressure)
    }

    pub(crate) fn memory_pressure_observation(&self) -> MemoryPressureObservation {
        self.memory_pressure.observation()
    }

    pub(crate) fn subscribe_memory_pressure(
        &self,
    ) -> tokio::sync::watch::Receiver<MemoryPressureObservation> {
        self.memory_pressure.subscribe()
    }

    fn apply_memory_pressure(&self, mut input: RuntimeControlInput) -> RuntimeControlInput {
        let memory = self.memory_pressure.observation();
        if memory.available && memory.sample.limit_mb > 0 {
            input.memory_used_mb = memory.sample.used_mb;
            input.memory_limit_mb = memory.sample.limit_mb;
        } else if memory.failures > 0 {
            input.memory_used_mb = 1;
            input.memory_limit_mb = 1;
        }
        input
    }

    #[cfg(test)]
    pub(crate) fn observe_and_apply(
        &self,
        input: RuntimeControlInput,
        apply: impl FnOnce(&AutoscaleDecision, bool),
    ) -> AutoscaleDecision {
        match self
            .apply(
                RuntimeControlOperation::Observe(input),
                |outcome, proposed| {
                    let RuntimeControlOutcome::Autoscale(decision) = outcome else {
                        unreachable!("tick observation returns autoscale outcome");
                    };
                    apply(decision, proposed.draining);
                    Ok(())
                },
            )
            .expect("test-only observation applicator is infallible")
        {
            RuntimeControlOutcome::Autoscale(decision) => decision,
            RuntimeControlOutcome::Work(_) => {
                unreachable!("tick observation returns autoscale outcome")
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn observe_signal_and_apply(
        &self,
        signal: RuntimeControlSignal,
        apply: impl FnOnce(&AutoscaleDecision, bool),
    ) -> AutoscaleDecision {
        match self
            .apply(
                RuntimeControlOperation::ObserveSignal(signal),
                |outcome, proposed| {
                    let RuntimeControlOutcome::Autoscale(decision) = outcome else {
                        unreachable!("signal observation returns autoscale outcome");
                    };
                    apply(decision, proposed.draining);
                    Ok(())
                },
            )
            .expect("test-only signal applicator is infallible")
        {
            RuntimeControlOutcome::Autoscale(decision) => decision,
            RuntimeControlOutcome::Work(_) => {
                unreachable!("signal observation returns autoscale outcome")
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn request_drain_and_apply(
        &self,
        apply: impl FnOnce(&AutoscaleDecision, bool),
    ) -> AutoscaleDecision {
        match self
            .apply(
                RuntimeControlOperation::RequestDrain,
                |outcome, proposed| {
                    let RuntimeControlOutcome::Autoscale(decision) = outcome else {
                        unreachable!("drain returns autoscale outcome");
                    };
                    apply(decision, proposed.draining);
                    Ok(())
                },
            )
            .expect("test-only drain applicator is infallible")
        {
            RuntimeControlOutcome::Autoscale(decision) => decision,
            RuntimeControlOutcome::Work(_) => unreachable!("drain returns autoscale outcome"),
        }
    }

    pub(crate) fn apply(
        &self,
        operation: RuntimeControlOperation,
        applicator: impl FnOnce(
            &RuntimeControlOutcome,
            &RuntimeControlSnapshot,
        ) -> Result<(), RuntimeControlApplyError>,
    ) -> Result<RuntimeControlOutcome, RuntimeControlApplyError> {
        let mut controller = self
            .controller
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(reason) = controller.application_stop_reason.clone() {
            return Err(RuntimeControlApplyError::controlled_stop(reason));
        }

        let previous = controller.clone();
        let outcome = match operation {
            RuntimeControlOperation::Observe(input) => {
                let input = self.apply_memory_pressure(input);
                RuntimeControlOutcome::Autoscale(controller.decide_observation(input))
            }
            RuntimeControlOperation::ObserveSignal(signal) => {
                RuntimeControlOutcome::Autoscale(controller.decide_signal(signal))
            }
            RuntimeControlOperation::ObserveWork(input) => {
                RuntimeControlOutcome::Work(controller.decide_work(input))
            }
            RuntimeControlOperation::RequestDrain => {
                RuntimeControlOutcome::Autoscale(controller.decide_drain())
            }
        };
        let proposed = controller.snapshot();
        let application = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            applicator(&outcome, &proposed)
        }));

        match application {
            Ok(Ok(())) => Ok(outcome),
            Ok(Err(error)) => {
                *controller = previous;
                let RuntimeControlApplyError::ControlledStop { reason } = &error;
                controller.application_stop_reason = Some(reason.clone());
                Err(error)
            }
            Err(panic) => {
                *controller = previous;
                controller.application_stop_reason =
                    Some("runtime-control applicator panicked".to_string());
                drop(controller);
                std::panic::resume_unwind(panic);
            }
        }
    }

    fn with_controller<T>(&self, f: impl FnOnce(&mut RuntimeControlPlane) -> T) -> T {
        let mut controller = self
            .controller
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f(&mut controller)
    }
}

pub(crate) fn autoscale_action_label(action: AutoscaleAction) -> &'static str {
    match action {
        AutoscaleAction::Hold => "hold",
        AutoscaleAction::ScaleDown => "scale_down",
        AutoscaleAction::ScaleUp => "scale_up",
    }
}

pub(crate) fn autoscale_pressure_label(pressure: Option<AutoscalePressure>) -> &'static str {
    match pressure {
        None => "none",
        Some(AutoscalePressure::TickTime) => "tick_time",
        Some(AutoscalePressure::ChunkQueue) => "chunk_queue",
        Some(AutoscalePressure::SlowClientShed) => "slow_client_shed",
        Some(AutoscalePressure::Memory) => "memory",
        Some(AutoscalePressure::FirstChunkSla) => "first_chunk_sla",
    }
}

fn halve_floor(value: u32, floor: u32) -> u32 {
    value.saturating_sub(value / 2).max(floor)
}

fn double_ceiling(value: u32, ceiling: u32) -> u32 {
    value.saturating_mul(2).min(ceiling)
}

fn halve_floor_usize(value: usize, floor: usize) -> usize {
    value.saturating_sub(value / 2).max(floor)
}

fn double_ceiling_usize(value: usize, ceiling: usize) -> usize {
    value.saturating_mul(2).min(ceiling)
}

fn recover_toward_ceiling_usize(value: usize, ceiling: usize) -> usize {
    value.saturating_add(ceiling.saturating_sub(value).div_ceil(2))
}

fn percent_at_least_u64(value: u64, capacity: u64, threshold: u8) -> bool {
    capacity > 0 && value.saturating_mul(100) >= capacity.saturating_mul(threshold as u64)
}

fn percent_at_most_u64(value: u64, capacity: u64, threshold: u8) -> bool {
    capacity > 0 && value.saturating_mul(100) <= capacity.saturating_mul(threshold as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn balanced_controller() -> RuntimeControlPlane {
        RuntimeControlPlane::new(
            AutoscalePolicy {
                scale_down_after_ticks: 2,
                scale_up_after_ticks: 3,
                ..AutoscalePolicy::for_profile(AutoscaleProfile::Balanced)
            },
            RuntimeControlLimits {
                view_distance: 8,
                chunk_send_rate: 16,
                chunk_load_rate: 32,
                chunk_generate_rate: 16,
            },
        )
    }

    fn healthy_input() -> RuntimeControlInput {
        RuntimeControlInput {
            tick_ms: 35,
            memory_used_mb: 512,
            memory_limit_mb: 4096,
        }
    }

    fn transactional_control() -> RuntimeControlHandle {
        RuntimeControlHandle::new(RuntimeControlConfig {
            policy: AutoscalePolicy {
                scale_up_after_ticks: 1,
                ..AutoscalePolicy::for_profile(AutoscaleProfile::Balanced)
            },
            initial_limits: RuntimeControlLimits {
                view_distance: 6,
                chunk_send_rate: 8,
                chunk_load_rate: 16,
                chunk_generate_rate: 8,
            },
        })
    }

    fn autoscale_outcome(outcome: &RuntimeControlOutcome) -> &AutoscaleDecision {
        match outcome {
            RuntimeControlOutcome::Autoscale(decision) => decision,
            RuntimeControlOutcome::Work(_) => panic!("expected autoscale outcome"),
        }
    }

    #[test]
    fn failed_application_restores_prior_controller_state_before_stop() {
        let control = transactional_control();
        let before = control.snapshot();

        let failed = control.apply(
            RuntimeControlOperation::Observe(healthy_input()),
            |outcome, proposed| {
                let decision = autoscale_outcome(outcome);
                assert_eq!(decision.action, AutoscaleAction::ScaleUp);
                assert_eq!(proposed.limits.view_distance, 7);
                Err(RuntimeControlApplyError::controlled_stop(
                    "owner lanes rejected target",
                ))
            },
        );

        assert_eq!(
            failed,
            Err(RuntimeControlApplyError::controlled_stop(
                "owner lanes rejected target"
            ))
        );
        let mut expected = before;
        expected.application_stop_reason = Some("owner lanes rejected target".to_owned());
        assert_eq!(control.snapshot(), expected);
    }

    #[test]
    fn failed_work_application_restores_prior_budgets_before_stop() {
        let control = transactional_control();
        let before = control.snapshot();
        let input = RuntimeWorkInput {
            tick_p95_us: 80_000,
            entity_goals_p95_us: 1_000,
            entity_physics_p95_us: 1_000,
            entity_dispatch_p95_us: 1_000,
            random_tick_p95_us: 30_000,
            block_tick_p95_us: 2_000,
            fluid_tick_p95_us: 2_000,
            scheduled_budget_exhausted: false,
        };

        let failed = control.apply(
            RuntimeControlOperation::ObserveWork(input),
            |outcome, proposed| {
                let RuntimeControlOutcome::Work(decision) = outcome else {
                    panic!("expected work outcome");
                };
                assert_eq!(decision.budgets.random_tick_chunks, 32);
                assert_eq!(proposed.work_budgets, decision.budgets);
                Err(RuntimeControlApplyError::controlled_stop(
                    "work-budget consumer rejected target",
                ))
            },
        );

        assert_eq!(
            failed,
            Err(RuntimeControlApplyError::controlled_stop(
                "work-budget consumer rejected target"
            ))
        );
        let mut expected = before;
        expected.application_stop_reason = Some("work-budget consumer rejected target".to_owned());
        assert_eq!(control.snapshot(), expected);
    }

    #[test]
    fn hold_after_drain_applies_one_coherent_cpu_and_lane_target() {
        #[derive(Debug)]
        struct AppliedResources {
            cpu_limit: usize,
            owner_lanes: usize,
            applications: usize,
        }

        let control = transactional_control();
        let mut resources = AppliedResources {
            cpu_limit: 2,
            owner_lanes: 2,
            applications: 0,
        };
        let mut apply = |outcome: &RuntimeControlOutcome,
                         proposed: &RuntimeControlSnapshot|
         -> Result<(), RuntimeControlApplyError> {
            let decision = autoscale_outcome(outcome);
            let target = if proposed.draining {
                1
            } else {
                match decision.action {
                    AutoscaleAction::ScaleDown => resources.cpu_limit.saturating_sub(1).max(1),
                    AutoscaleAction::ScaleUp => resources.cpu_limit.saturating_add(1),
                    AutoscaleAction::Hold => resources.cpu_limit,
                }
            };
            resources.cpu_limit = target;
            resources.owner_lanes = target;
            resources.applications += 1;
            Ok(())
        };

        let drain = control
            .apply(RuntimeControlOperation::RequestDrain, &mut apply)
            .expect("drain application succeeds");
        assert_eq!(autoscale_outcome(&drain).action, AutoscaleAction::ScaleDown);

        let hold = control
            .apply(
                RuntimeControlOperation::Observe(healthy_input()),
                &mut apply,
            )
            .expect("drain hold application succeeds");
        assert_eq!(autoscale_outcome(&hold).action, AutoscaleAction::Hold);
        assert_eq!(resources.cpu_limit, 1);
        assert_eq!(resources.owner_lanes, 1);
        assert_eq!(resources.applications, 2);
    }

    #[test]
    fn concurrent_observations_apply_in_exact_controller_order() {
        use std::sync::mpsc;

        let control = Arc::new(transactional_control());
        let events = Arc::new(Mutex::new(Vec::new()));
        let (first_started_tx, first_started_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();

        let first = {
            let control = Arc::clone(&control);
            let events = Arc::clone(&events);
            std::thread::spawn(move || {
                control.apply(
                    RuntimeControlOperation::Observe(healthy_input()),
                    |outcome, proposed| {
                        let decision = autoscale_outcome(outcome);
                        assert_eq!(decision.limits.view_distance, 7);
                        assert_eq!(proposed.limits.view_distance, 7);
                        events
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push("first application started");
                        first_started_tx.send(()).expect("first start is observed");
                        release_first_rx
                            .recv()
                            .expect("first application is released");
                        events
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push("first application finished");
                        Ok(())
                    },
                )
            })
        };
        first_started_rx.recv().expect("first application starts");

        let (second_attempt_tx, second_attempt_rx) = mpsc::channel();
        let second = {
            let control = Arc::clone(&control);
            let events = Arc::clone(&events);
            std::thread::spawn(move || {
                second_attempt_tx
                    .send(())
                    .expect("second attempt is observed");
                control.apply(
                    RuntimeControlOperation::Observe(healthy_input()),
                    |outcome, proposed| {
                        let decision = autoscale_outcome(outcome);
                        assert_eq!(decision.limits.view_distance, 8);
                        assert_eq!(proposed.limits.view_distance, 8);
                        events
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push("second application");
                        Ok(())
                    },
                )
            })
        };
        second_attempt_rx
            .recv()
            .expect("second transaction attempts entry");
        release_first_tx
            .send(())
            .expect("release first application");

        first
            .join()
            .expect("first observation thread joins")
            .expect("first observation applies");
        second
            .join()
            .expect("second observation thread joins")
            .expect("second observation applies");

        assert_eq!(
            *events
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            [
                "first application started",
                "first application finished",
                "second application",
            ]
        );
        assert_eq!(control.snapshot().limits.view_distance, 8);
    }

    #[test]
    fn uncertain_application_fences_later_controller_mutation() {
        use std::sync::atomic::{AtomicBool, Ordering};

        let control = transactional_control();
        let before = control.snapshot();
        let stopped = RuntimeControlApplyError::controlled_stop("owner lane outcome unknown");

        let result = control.apply(RuntimeControlOperation::Observe(healthy_input()), |_, _| {
            Err(stopped.clone())
        });
        assert_eq!(result, Err(stopped.clone()));

        let mut expected = before;
        expected.application_stop_reason = Some("owner lane outcome unknown".to_string());
        assert_eq!(control.snapshot(), expected);

        let called = AtomicBool::new(false);
        let later = control.apply(RuntimeControlOperation::RequestDrain, |_, _| {
            called.store(true, Ordering::SeqCst);
            Ok(())
        });
        assert_eq!(later, Err(stopped));
        assert!(!called.load(Ordering::SeqCst));
        assert!(!control.snapshot().draining);
    }

    #[test]
    fn panicking_applicator_restores_prior_state_and_fences_controller() {
        let control = transactional_control();
        let before = control.snapshot();

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = control.apply(RuntimeControlOperation::Observe(healthy_input()), |_, _| {
                panic!("application panic");
            });
        }));

        assert!(panic.is_err());
        let mut expected = before;
        expected.application_stop_reason = Some("runtime-control applicator panicked".to_string());
        assert_eq!(control.snapshot(), expected);
        assert_eq!(
            control.apply(RuntimeControlOperation::RequestDrain, |_, _| Ok(())),
            Err(RuntimeControlApplyError::controlled_stop(
                "runtime-control applicator panicked"
            ))
        );
    }

    #[tokio::test]
    async fn runtime_control_signal_push_wakes_waiting_consumer() {
        let (producer, mut consumer) = runtime_control_signal_channel();
        let mut chunk_pressure = producer.chunk_pressure_source();
        let (waiting, waiting_rx) = tokio::sync::oneshot::channel();
        let (received, received_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            waiting.send(()).expect("test observes receiver wait");
            received
                .send(consumer.recv().await)
                .expect("test observes received signal");
        });
        waiting_rx.await.expect("receiver starts waiting");

        assert!(chunk_pressure.set_saturated(true));
        assert_eq!(
            received_rx.await.expect("receiver wakes"),
            Some(RuntimeControlSignal::ChunkPressure {
                saturated_sources: 1,
            })
        );
    }

    #[tokio::test]
    async fn runtime_control_signal_full_drain_then_park_cannot_miss_push() {
        let (producer, mut consumer) = runtime_control_signal_channel();

        assert!(producer.push_slow_client_shed());
        assert_eq!(
            consumer.recv().await,
            Some(RuntimeControlSignal::SlowClientShed)
        );

        let (registered, registered_rx) = tokio::sync::oneshot::channel();
        let (resume, resume_rx) = tokio::sync::oneshot::channel();
        let receive = tokio::spawn(async move {
            consumer
                .recv_after_registration(async move {
                    registered
                        .send(())
                        .expect("test observes notification registration");
                    resume_rx.await.expect("test resumes state check");
                })
                .await
        });
        registered_rx
            .await
            .expect("receiver registers before checking drained state");

        assert!(producer.push_slow_client_shed());
        resume.send(()).expect("receiver resumes");

        assert_eq!(
            receive.await.expect("receiver task joins"),
            Some(RuntimeControlSignal::SlowClientShed)
        );
    }

    #[tokio::test]
    async fn chunk_pressure_sources_recover_independently() {
        let (producer, mut consumer) = runtime_control_signal_channel();
        let mut first = producer.chunk_pressure_source();
        let mut second = producer.chunk_pressure_source();

        assert!(first.set_saturated(true));
        assert_eq!(
            consumer.recv().await,
            Some(RuntimeControlSignal::ChunkPressure {
                saturated_sources: 1,
            })
        );
        assert!(second.set_saturated(true));
        assert_eq!(
            consumer.recv().await,
            Some(RuntimeControlSignal::ChunkPressure {
                saturated_sources: 2,
            })
        );
        assert!(first.set_saturated(false));
        assert_eq!(
            consumer.recv().await,
            Some(RuntimeControlSignal::ChunkPressure {
                saturated_sources: 1,
            })
        );
        assert!(second.set_saturated(false));
        assert_eq!(
            consumer.recv().await,
            Some(RuntimeControlSignal::ChunkPressure {
                saturated_sources: 0,
            })
        );
    }

    #[tokio::test]
    async fn first_chunk_sla_sources_recover_independently() {
        let (producer, mut consumer) = runtime_control_signal_channel();
        let mut first = producer.first_chunk_sla_source();
        let mut second = producer.first_chunk_sla_source();

        assert!(first.set_active(true));
        assert_eq!(
            consumer.recv().await,
            Some(RuntimeControlSignal::FirstChunkSla { active_sources: 1 })
        );
        assert!(second.set_active(true));
        assert_eq!(
            consumer.recv().await,
            Some(RuntimeControlSignal::FirstChunkSla { active_sources: 2 })
        );
        assert!(first.set_active(false));
        assert_eq!(
            consumer.recv().await,
            Some(RuntimeControlSignal::FirstChunkSla { active_sources: 1 })
        );
        drop(second);
        assert_eq!(
            consumer.recv().await,
            Some(RuntimeControlSignal::FirstChunkSla { active_sources: 0 })
        );
    }

    #[test]
    fn first_chunk_recovery_does_not_clear_chunk_pressure() {
        let mut controller = balanced_controller();
        controller.observe_signal(RuntimeControlSignal::ChunkPressure {
            saturated_sources: 1,
        });
        controller.observe_signal(RuntimeControlSignal::FirstChunkSla { active_sources: 1 });

        let decision =
            controller.observe_signal(RuntimeControlSignal::FirstChunkSla { active_sources: 0 });

        assert_eq!(decision.pressure, Some(AutoscalePressure::ChunkQueue));
        assert_eq!(controller.active_chunk_saturations, 1);
        assert_eq!(controller.active_first_chunk_sla_sources, 0);
    }

    #[test]
    fn controller_recovers_only_after_last_chunk_pressure_source() {
        let mut controller = balanced_controller();

        controller.observe_signal(RuntimeControlSignal::ChunkPressure {
            saturated_sources: 2,
        });
        let first_recovery = controller.observe_signal(RuntimeControlSignal::ChunkPressure {
            saturated_sources: 1,
        });
        assert_eq!(first_recovery.pressure, Some(AutoscalePressure::ChunkQueue));
        assert_eq!(first_recovery.action, AutoscaleAction::ScaleDown);

        let last_recovery = controller.observe_signal(RuntimeControlSignal::ChunkPressure {
            saturated_sources: 0,
        });
        assert_eq!(last_recovery.action, AutoscaleAction::Hold);
        assert_eq!(last_recovery.pressure, None);
        assert_eq!(controller.snapshot().pressure_ticks, 0);
        assert_eq!(controller.snapshot().healthy_ticks, 0);
    }

    #[test]
    fn sustained_chunk_saturation_survives_zero_depth_tick_observations() {
        let mut controller = balanced_controller();

        let transition = controller.observe_signal(RuntimeControlSignal::ChunkPressure {
            saturated_sources: 1,
        });
        assert_eq!(transition.action, AutoscaleAction::Hold);
        assert_eq!(transition.pressure, Some(AutoscalePressure::ChunkQueue));

        let sustained = controller.observe(healthy_input());
        assert_eq!(sustained.action, AutoscaleAction::ScaleDown);
        assert_eq!(sustained.pressure, Some(AutoscalePressure::ChunkQueue));
        assert_eq!(controller.snapshot().healthy_ticks, 0);
    }

    #[test]
    fn runtime_control_drain_precedes_producer_pressure() {
        let mut controller = balanced_controller();

        let drain = controller.request_drain();
        let after_signal = controller.observe_signal(RuntimeControlSignal::ChunkPressure {
            saturated_sources: 1,
        });

        assert_eq!(after_signal.action, AutoscaleAction::Hold);
        assert_eq!(after_signal.limits, drain.limits);
        assert!(controller.snapshot().draining);
    }

    #[test]
    fn runtime_control_signal_producer_reports_closed_consumer() {
        let (producer, consumer) = runtime_control_signal_channel();
        drop(consumer);

        assert!(!producer.push_slow_client_shed());
    }

    #[test]
    fn autoscale_application_then_drain_finishes_at_drain_limits() {
        use std::sync::Barrier;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let control = Arc::new(RuntimeControlHandle::new(RuntimeControlConfig {
            policy: AutoscalePolicy {
                scale_up_after_ticks: 1,
                ..AutoscalePolicy::for_profile(AutoscaleProfile::Balanced)
            },
            initial_limits: RuntimeControlLimits {
                view_distance: 6,
                chunk_send_rate: 8,
                chunk_load_rate: 16,
                chunk_generate_rate: 8,
            },
        }));
        let applied_limit = Arc::new(AtomicUsize::new(1));
        let scale_apply_started = Arc::new(Barrier::new(2));
        let release_scale_apply = Arc::new(Barrier::new(2));
        let drain_attempting = Arc::new(Barrier::new(2));

        let scale_thread = {
            let control = Arc::clone(&control);
            let applied_limit = Arc::clone(&applied_limit);
            let scale_apply_started = Arc::clone(&scale_apply_started);
            let release_scale_apply = Arc::clone(&release_scale_apply);
            std::thread::spawn(move || {
                control.observe_and_apply(healthy_input(), |decision, draining| {
                    assert_eq!(decision.action, AutoscaleAction::ScaleUp);
                    assert!(!draining);
                    scale_apply_started.wait();
                    release_scale_apply.wait();
                    applied_limit.store(2, Ordering::SeqCst);
                });
            })
        };
        scale_apply_started.wait();

        let drain_thread = {
            let control = Arc::clone(&control);
            let applied_limit = Arc::clone(&applied_limit);
            let drain_attempting = Arc::clone(&drain_attempting);
            std::thread::spawn(move || {
                drain_attempting.wait();
                control.request_drain_and_apply(|decision, draining| {
                    assert_eq!(decision.action, AutoscaleAction::ScaleDown);
                    assert!(draining);
                    applied_limit.store(1, Ordering::SeqCst);
                });
            })
        };
        drain_attempting.wait();
        release_scale_apply.wait();
        scale_thread.join().expect("scale thread joins");
        drain_thread.join().expect("drain thread joins");

        assert_eq!(applied_limit.load(Ordering::SeqCst), 1);
        assert!(control.snapshot().draining);
    }

    #[test]
    fn drain_application_then_autoscale_cannot_raise_limits() {
        use std::sync::Barrier;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let control = Arc::new(RuntimeControlHandle::new(RuntimeControlConfig {
            policy: AutoscalePolicy {
                scale_up_after_ticks: 1,
                ..AutoscalePolicy::for_profile(AutoscaleProfile::Balanced)
            },
            initial_limits: RuntimeControlLimits {
                view_distance: 6,
                chunk_send_rate: 8,
                chunk_load_rate: 16,
                chunk_generate_rate: 8,
            },
        }));
        let applied_limit = Arc::new(AtomicUsize::new(2));
        let drain_apply_started = Arc::new(Barrier::new(2));
        let release_drain_apply = Arc::new(Barrier::new(2));
        let scale_attempting = Arc::new(Barrier::new(2));

        let drain_thread = {
            let control = Arc::clone(&control);
            let applied_limit = Arc::clone(&applied_limit);
            let drain_apply_started = Arc::clone(&drain_apply_started);
            let release_drain_apply = Arc::clone(&release_drain_apply);
            std::thread::spawn(move || {
                control.request_drain_and_apply(|decision, draining| {
                    assert_eq!(decision.action, AutoscaleAction::ScaleDown);
                    assert!(draining);
                    drain_apply_started.wait();
                    release_drain_apply.wait();
                    applied_limit.store(1, Ordering::SeqCst);
                });
            })
        };
        drain_apply_started.wait();

        let scale_thread = {
            let control = Arc::clone(&control);
            let applied_limit = Arc::clone(&applied_limit);
            let scale_attempting = Arc::clone(&scale_attempting);
            std::thread::spawn(move || {
                scale_attempting.wait();
                control.observe_and_apply(healthy_input(), |decision, draining| {
                    assert_eq!(decision.action, AutoscaleAction::Hold);
                    assert!(draining);
                    if decision.action == AutoscaleAction::ScaleUp {
                        applied_limit.store(2, Ordering::SeqCst);
                    }
                });
            })
        };
        scale_attempting.wait();
        release_drain_apply.wait();
        drain_thread.join().expect("drain thread joins");
        scale_thread.join().expect("scale thread joins");

        assert_eq!(applied_limit.load(Ordering::SeqCst), 1);
        assert!(control.snapshot().draining);
    }

    #[test]
    fn pressure_requires_hysteresis_before_scaling_down() {
        let mut controller = balanced_controller();
        let input = RuntimeControlInput {
            tick_ms: 80,
            ..healthy_input()
        };

        let first = controller.observe(input);
        assert_eq!(first.action, AutoscaleAction::Hold);
        assert_eq!(first.pressure, Some(AutoscalePressure::TickTime));

        let second = controller.observe(input);
        assert_eq!(second.action, AutoscaleAction::ScaleDown);
        assert_eq!(second.limits.view_distance, 7);
        assert_eq!(second.limits.chunk_send_rate, 8);
        assert_eq!(
            second.reason,
            "pressure persisted for 2 observations; applying bounded degradation"
        );

        let cooldown = controller.observe(input);
        assert_eq!(cooldown.action, AutoscaleAction::Hold);
        assert_eq!(cooldown.limits, second.limits);
    }

    #[test]
    fn first_runtime_pressure_immediately_yields_random_tick_work() {
        let mut controller = balanced_controller();

        let decision = controller.observe_signal(RuntimeControlSignal::ChunkPressure {
            saturated_sources: 1,
        });

        assert_eq!(decision.action, AutoscaleAction::Hold);
        assert_eq!(decision.pressure, Some(AutoscalePressure::ChunkQueue));
        assert_eq!(controller.snapshot().work_budgets.random_tick_chunks, 32);
        assert_eq!(controller.snapshot().work_budgets.scheduled_ticks, 256);
    }

    #[test]
    fn healthy_ticks_restore_throughput_without_overshooting_bounds() {
        let mut controller = balanced_controller();
        controller.observe_signal(RuntimeControlSignal::ChunkPressure {
            saturated_sources: 1,
        });
        controller.observe(healthy_input());
        controller.observe_signal(RuntimeControlSignal::ChunkPressure {
            saturated_sources: 0,
        });

        assert_eq!(controller.snapshot().limits.view_distance, 7);
        assert_eq!(
            controller.observe(healthy_input()).action,
            AutoscaleAction::Hold
        );
        assert_eq!(
            controller.observe(healthy_input()).action,
            AutoscaleAction::Hold
        );
        let restored = controller.observe(healthy_input());

        assert_eq!(restored.action, AutoscaleAction::ScaleUp);
        assert_eq!(restored.limits.view_distance, 8);
        assert_eq!(restored.limits.chunk_send_rate, 16);
        assert_eq!(
            restored.reason,
            "healthy for 3 observations; restoring bounded throughput"
        );

        let cooldown = controller.observe(healthy_input());
        assert_eq!(cooldown.action, AutoscaleAction::Hold);
        assert_eq!(cooldown.limits, restored.limits);
    }

    #[test]
    fn target_boundary_does_not_restore_capacity_without_headroom() {
        let mut controller = balanced_controller();
        let pressure = RuntimeControlInput {
            tick_ms: 80,
            ..healthy_input()
        };
        controller.observe(pressure);
        let reduced = controller.observe(pressure);
        assert_eq!(reduced.action, AutoscaleAction::ScaleDown);

        let deadband = RuntimeControlInput {
            tick_ms: 45,
            ..healthy_input()
        };
        for _ in 0..30 {
            let held = controller.observe(deadband);
            assert_eq!(held.action, AutoscaleAction::Hold);
            assert_eq!(held.limits, reduced.limits);
        }
        assert_eq!(controller.snapshot().healthy_ticks, 0);

        assert_eq!(
            controller.observe(healthy_input()).action,
            AutoscaleAction::Hold
        );
        assert_eq!(
            controller.observe(healthy_input()).action,
            AutoscaleAction::Hold
        );
        assert_eq!(
            controller.observe(healthy_input()).action,
            AutoscaleAction::ScaleUp
        );
    }

    #[test]
    fn memory_pressure_takes_priority_over_a_full_chunk_queue() {
        let mut controller = balanced_controller();
        controller.observe_signal(RuntimeControlSignal::ChunkPressure {
            saturated_sources: 1,
        });

        let decision = controller.observe(RuntimeControlInput {
            memory_used_mb: 900,
            memory_limit_mb: 1_000,
            ..healthy_input()
        });

        assert_eq!(decision.pressure, Some(AutoscalePressure::Memory));
    }

    #[test]
    fn drain_clamps_to_minimum_and_stays_observable() {
        let mut controller = balanced_controller();

        let drain = controller.request_drain();
        assert_eq!(drain.action, AutoscaleAction::ScaleDown);
        assert_eq!(drain.limits.view_distance, 6);
        assert!(drain.reason.contains("drain requested"));

        let after = controller.observe(healthy_input());
        assert_eq!(after.action, AutoscaleAction::Hold);
        assert_eq!(after.limits, drain.limits);
        assert!(controller.snapshot().draining);
    }

    #[test]
    fn runtime_control_handle_applies_memory_snapshot_to_all_observations() {
        let memory_pressure = crate::memory_pressure::MemoryPressureHandle::with_sample(
            crate::memory_pressure::MemoryPressureSnapshot {
                used_mb: 900,
                limit_mb: 1_000,
            },
        );
        let control = RuntimeControlHandle::new_with_memory_pressure(
            RuntimeControlConfig {
                policy: AutoscalePolicy {
                    memory_pressure_percent: 50,
                    scale_down_after_ticks: 1,
                    ..AutoscalePolicy::for_profile(AutoscaleProfile::Balanced)
                },
                initial_limits: RuntimeControlLimits {
                    view_distance: 8,
                    chunk_send_rate: 16,
                    chunk_load_rate: 32,
                    chunk_generate_rate: 16,
                },
            },
            memory_pressure,
        );

        let decision = control.observe(RuntimeControlInput {
            memory_used_mb: 0,
            memory_limit_mb: 0,
            ..healthy_input()
        });

        assert_eq!(decision.action, AutoscaleAction::ScaleDown);
        assert_eq!(decision.pressure, Some(AutoscalePressure::Memory));
        assert_eq!(decision.limits.view_distance, 7);
    }

    #[test]
    fn failed_memory_sample_applies_conservative_pressure() {
        let memory_pressure = crate::memory_pressure::MemoryPressureHandle::with_sample(
            crate::memory_pressure::MemoryPressureSnapshot {
                used_mb: 100,
                limit_mb: 1_000,
            },
        );
        memory_pressure.fail_sample_for_test();
        let control = RuntimeControlHandle::new_with_memory_pressure(
            RuntimeControlConfig {
                policy: AutoscalePolicy {
                    memory_pressure_percent: 90,
                    scale_down_after_ticks: 1,
                    ..AutoscalePolicy::for_profile(AutoscaleProfile::Balanced)
                },
                initial_limits: RuntimeControlLimits {
                    view_distance: 8,
                    chunk_send_rate: 16,
                    chunk_load_rate: 32,
                    chunk_generate_rate: 16,
                },
            },
            memory_pressure,
        );

        let decision = control.observe(RuntimeControlInput {
            memory_used_mb: 0,
            memory_limit_mb: 0,
            ..healthy_input()
        });

        assert_eq!(decision.pressure, Some(AutoscalePressure::Memory));
        assert_eq!(decision.action, AutoscaleAction::ScaleDown);
    }

    #[test]
    fn runtime_control_unknown_memory_snapshot_is_inert() {
        let control = RuntimeControlHandle::new_with_memory_pressure(
            RuntimeControlConfig {
                policy: AutoscalePolicy {
                    memory_pressure_percent: 1,
                    scale_down_after_ticks: 1,
                    ..AutoscalePolicy::for_profile(AutoscaleProfile::Balanced)
                },
                initial_limits: RuntimeControlLimits {
                    view_distance: 8,
                    chunk_send_rate: 16,
                    chunk_load_rate: 32,
                    chunk_generate_rate: 16,
                },
            },
            crate::memory_pressure::MemoryPressureHandle::default(),
        );

        let decision = control.observe(RuntimeControlInput {
            memory_used_mb: 0,
            memory_limit_mb: 0,
            ..healthy_input()
        });

        assert_eq!(decision.action, AutoscaleAction::Hold);
        assert_eq!(decision.pressure, None);
        assert_eq!(decision.limits.view_distance, 8);
    }

    #[test]
    fn policies_normalize_invalid_bounds() {
        let policy = AutoscalePolicy {
            max_view_distance: 1,
            min_view_distance: 0,
            max_chunk_send_rate: 0,
            min_chunk_send_rate: 0,
            queue_pressure_percent: 0,
            memory_pressure_percent: 0,
            scale_down_after_ticks: 0,
            scale_up_after_ticks: 0,
            ..AutoscalePolicy::for_profile(AutoscaleProfile::LowEnd)
        }
        .normalized();

        assert_eq!(policy.min_view_distance, 2);
        assert_eq!(policy.max_view_distance, 2);
        assert_eq!(policy.min_chunk_send_rate, 1);
        assert_eq!(policy.max_chunk_send_rate, 1);
        assert_eq!(policy.queue_pressure_percent, 1);
        assert_eq!(policy.memory_pressure_percent, 1);
        assert_eq!(policy.scale_down_after_ticks, 1);
        assert_eq!(policy.scale_up_after_ticks, 1);
    }

    #[test]
    fn policies_cap_view_distance_at_vanilla_limit() {
        let policy = AutoscalePolicy {
            min_view_distance: 4,
            max_view_distance: i32::MAX,
            ..AutoscalePolicy::for_profile(AutoscaleProfile::LowEnd)
        }
        .normalized();

        assert_eq!(policy.min_view_distance, 4);
        assert_eq!(policy.max_view_distance, 32);
    }

    #[test]
    fn high_end_profile_scales_view_distance_between_eight_and_thirty_two() {
        let policy = AutoscalePolicy {
            scale_down_after_ticks: 1,
            scale_up_after_ticks: 1,
            ..AutoscalePolicy::for_profile(AutoscaleProfile::HighEnd)
        };
        assert_eq!(policy.min_view_distance, 8);
        assert_eq!(policy.max_view_distance, 32);

        let mut controller = RuntimeControlPlane::new(
            policy,
            RuntimeControlLimits {
                view_distance: 32,
                chunk_send_rate: 32,
                chunk_load_rate: 96,
                chunk_generate_rate: 64,
            },
        );
        let pressured = RuntimeControlInput {
            tick_ms: 51,
            memory_used_mb: 512,
            memory_limit_mb: 4096,
        };
        for expected in (8..32).rev() {
            let decision = controller.observe(pressured);
            assert_eq!(decision.action, AutoscaleAction::ScaleDown);
            assert_eq!(decision.pressure, Some(AutoscalePressure::TickTime));
            assert_eq!(decision.limits.view_distance, expected);
        }
        let floor = controller.observe(pressured);
        assert_eq!(floor.action, AutoscaleAction::Hold);
        assert_eq!(floor.limits.view_distance, 8);

        for expected in 9..=32 {
            let decision = controller.observe(healthy_input());
            assert_eq!(decision.action, AutoscaleAction::ScaleUp);
            assert_eq!(decision.limits.view_distance, expected);
        }
        let ceiling = controller.observe(healthy_input());
        assert_eq!(ceiling.action, AutoscaleAction::Hold);
        assert_eq!(ceiling.limits.view_distance, 32);
    }

    #[test]
    fn work_pressure_reduces_the_measured_expensive_random_tick_budget() {
        let mut controller = balanced_controller();

        let decision = controller.observe_work(RuntimeWorkInput {
            tick_p95_us: 60_000,
            entity_goals_p95_us: 1_000,
            entity_physics_p95_us: 1_000,
            entity_dispatch_p95_us: 1_000,
            random_tick_p95_us: 18_000,
            block_tick_p95_us: 2_000,
            fluid_tick_p95_us: 1_000,
            scheduled_budget_exhausted: false,
        });

        assert_eq!(decision.action, AutoscaleAction::ScaleDown);
        assert_eq!(decision.focus, Some(RuntimeWorkFocus::RandomTicks));
        assert_eq!(decision.budgets.random_tick_chunks, 32);
        assert_eq!(decision.budgets.scheduled_ticks, 256);
    }

    #[test]
    fn work_pressure_reduces_entity_pathing_without_dropping_physics() {
        let mut controller = balanced_controller();

        let decision = controller.observe_work(RuntimeWorkInput {
            tick_p95_us: 60_000,
            entity_goals_p95_us: 12_000,
            entity_physics_p95_us: 18_000,
            entity_dispatch_p95_us: 8_000,
            random_tick_p95_us: 2_000,
            block_tick_p95_us: 2_000,
            fluid_tick_p95_us: 1_000,
            scheduled_budget_exhausted: false,
        });

        assert_eq!(decision.action, AutoscaleAction::ScaleDown);
        assert_eq!(decision.focus, Some(RuntimeWorkFocus::EntitySimulation));
        assert_eq!(decision.budgets.entity_pathing_candidates, 4);
        assert_eq!(decision.budgets.random_tick_chunks, 64);
        assert_eq!(decision.budgets.scheduled_ticks, 256);
    }

    #[test]
    fn work_pressure_attributes_dispatch_dominated_ticks_to_entity_simulation() {
        let mut controller = balanced_controller();

        let decision = controller.observe_work(RuntimeWorkInput {
            tick_p95_us: 60_000,
            entity_goals_p95_us: 1_000,
            entity_physics_p95_us: 1_000,
            entity_dispatch_p95_us: 20_000,
            random_tick_p95_us: 15_000,
            block_tick_p95_us: 2_000,
            fluid_tick_p95_us: 1_000,
            scheduled_budget_exhausted: false,
        });

        assert_eq!(decision.action, AutoscaleAction::ScaleDown);
        assert_eq!(decision.focus, Some(RuntimeWorkFocus::EntitySimulation));
        assert_eq!(decision.budgets.entity_pathing_candidates, 4);
    }

    #[test]
    fn scheduled_backlog_keeps_its_budget_and_yields_random_tick_work() {
        let mut controller = balanced_controller();

        let decision = controller.observe_work(RuntimeWorkInput {
            tick_p95_us: 60_000,
            entity_goals_p95_us: 1_000,
            entity_physics_p95_us: 1_000,
            entity_dispatch_p95_us: 1_000,
            random_tick_p95_us: 1_000,
            block_tick_p95_us: 30_000,
            fluid_tick_p95_us: 20_000,
            scheduled_budget_exhausted: true,
        });

        assert_eq!(decision.action, AutoscaleAction::ScaleDown);
        assert_eq!(decision.focus, Some(RuntimeWorkFocus::ScheduledTicks));
        assert_eq!(decision.budgets.random_tick_chunks, 32);
        assert_eq!(decision.budgets.scheduled_ticks, 256);
    }

    #[test]
    fn healthy_work_window_recovers_reduced_budget_without_jumping_to_maximum() {
        let mut controller = balanced_controller();
        controller.observe_work(RuntimeWorkInput {
            tick_p95_us: 60_000,
            entity_goals_p95_us: 1_000,
            entity_physics_p95_us: 1_000,
            entity_dispatch_p95_us: 1_000,
            random_tick_p95_us: 18_000,
            block_tick_p95_us: 2_000,
            fluid_tick_p95_us: 1_000,
            scheduled_budget_exhausted: false,
        });

        let decision = controller.observe_work(RuntimeWorkInput {
            tick_p95_us: 30_000,
            entity_goals_p95_us: 1_000,
            entity_physics_p95_us: 1_000,
            entity_dispatch_p95_us: 1_000,
            random_tick_p95_us: 1_000,
            block_tick_p95_us: 1_000,
            fluid_tick_p95_us: 1_000,
            scheduled_budget_exhausted: false,
        });

        assert_eq!(decision.action, AutoscaleAction::ScaleUp);
        assert_eq!(decision.budgets.random_tick_chunks, 48);
        assert_eq!(decision.budgets.scheduled_ticks, 256);
    }
}
