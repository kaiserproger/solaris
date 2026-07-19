//! Bounded runtime-control decisions for chunk throughput and view budgets.
//!
//! This module is deliberately local-process only. It makes per-instance
//! backpressure decisions observable; it does not coordinate shared-world
//! horizontal sharding.

use std::sync::{Arc, Mutex};

use crate::memory_pressure::{
    MemoryPressureHandle, MemoryPressureObservation, MemoryPressureSampler,
    spawn_memory_pressure_sampler,
};

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
                max_view_distance: 12,
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
    pub queued_chunks: usize,
    pub queue_capacity: usize,
    pub memory_used_mb: u64,
    pub memory_limit_mb: u64,
    pub first_chunk_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoscalePressure {
    TickTime,
    ChunkQueue,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeControlConfig {
    pub policy: AutoscalePolicy,
    pub initial_limits: RuntimeControlLimits,
}

#[derive(Debug, Clone)]
pub struct RuntimeControlHandle {
    controller: Arc<Mutex<RuntimeControlPlane>>,
    memory_pressure: MemoryPressureHandle,
}

#[derive(Debug, Clone)]
pub struct RuntimeControlPlane {
    policy: AutoscalePolicy,
    limits: RuntimeControlLimits,
    last_decision: AutoscaleDecision,
    pending_pressure: Option<AutoscalePressure>,
    work_bounds: RuntimeWorkBudgetBounds,
    work_budgets: RuntimeWorkBudgets,
    last_work_decision: RuntimeWorkDecision,
    pressure_ticks: u32,
    healthy_ticks: u32,
    draining: bool,
}

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
            pending_pressure: None,
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
            draining: false,
        }
    }

    pub fn request_drain(&mut self) -> AutoscaleDecision {
        self.draining = true;
        self.pending_pressure = None;
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

    pub fn observe(&mut self, input: RuntimeControlInput) -> AutoscaleDecision {
        if self.draining {
            return self.record(AutoscaleDecision {
                action: AutoscaleAction::Hold,
                pressure: None,
                limits: self.limits,
                reason: "drain active; holding minimum limits".to_string(),
            });
        }

        let pressure = strongest_pressure(self.pressure(input), self.pending_pressure.take());
        if let Some(kind) = pressure {
            self.yield_random_tick_work(kind);
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
                    pressure: Some(kind),
                    limits: self.limits,
                    reason: format!(
                        "pressure persisted for {} ticks; applying bounded degradation",
                        pressure_ticks
                    ),
                });
            }
            return self.record(AutoscaleDecision {
                action: AutoscaleAction::Hold,
                pressure: Some(kind),
                limits: self.limits,
                reason: format!(
                    "pressure observed for {} ticks; waiting for hysteresis",
                    self.pressure_ticks
                ),
            });
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
                    "healthy for {} ticks; restoring bounded throughput",
                    healthy_ticks
                ),
            });
        }

        self.record(AutoscaleDecision {
            action: AutoscaleAction::Hold,
            pressure: None,
            limits: self.limits,
            reason: format!(
                "healthy for {} ticks; waiting for hysteresis",
                self.healthy_ticks
            ),
        })
    }

    fn report_pressure(&mut self, input: RuntimeControlInput) -> AutoscaleDecision {
        let pressure = self.pressure(input);
        if !self.draining {
            self.pending_pressure = strongest_pressure(self.pending_pressure, pressure);
        }

        AutoscaleDecision {
            action: AutoscaleAction::Hold,
            pressure,
            limits: self.limits,
            reason: match pressure {
                Some(_) => "background pressure reported; tick owner controls hysteresis",
                None => "no background pressure; tick owner controls recovery",
            }
            .to_string(),
        }
    }

    pub fn observe_work(&mut self, input: RuntimeWorkInput) -> RuntimeWorkDecision {
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
        if input
            .first_chunk_ms
            .is_some_and(|ms| ms > self.policy.target_first_chunk_ms)
        {
            return Some(AutoscalePressure::FirstChunkSla);
        }
        if percent_at_least(
            input.queued_chunks,
            input.queue_capacity,
            self.policy.queue_pressure_percent,
        ) {
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
}

impl RuntimeControlHandle {
    #[must_use]
    pub fn new(config: RuntimeControlConfig) -> Self {
        Self {
            controller: Arc::new(Mutex::new(RuntimeControlPlane::new(
                config.policy,
                config.initial_limits,
            ))),
            memory_pressure: MemoryPressureHandle::default(),
        }
    }

    pub fn request_drain(&self) -> AutoscaleDecision {
        self.with_controller(RuntimeControlPlane::request_drain)
    }

    pub fn observe(&self, input: RuntimeControlInput) -> AutoscaleDecision {
        let input = self.with_memory_pressure(input);
        self.with_controller(|controller| controller.observe(input))
    }

    pub(crate) fn report_pressure(&self, input: RuntimeControlInput) -> AutoscaleDecision {
        let input = self.with_memory_pressure(input);
        self.with_controller(|controller| controller.report_pressure(input))
    }

    pub fn observe_work(&self, input: RuntimeWorkInput) -> RuntimeWorkDecision {
        self.with_controller(|controller| controller.observe_work(input))
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
        Self {
            controller: Arc::new(Mutex::new(RuntimeControlPlane::new(
                config.policy,
                config.initial_limits,
            ))),
            memory_pressure,
        }
    }

    pub(crate) fn memory_pressure_observation(&self) -> MemoryPressureObservation {
        self.memory_pressure.observation()
    }

    pub(crate) fn subscribe_memory_pressure(
        &self,
    ) -> tokio::sync::watch::Receiver<MemoryPressureObservation> {
        self.memory_pressure.subscribe()
    }

    fn with_memory_pressure(&self, mut input: RuntimeControlInput) -> RuntimeControlInput {
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

fn percent_at_least(value: usize, capacity: usize, threshold: u8) -> bool {
    capacity > 0 && value.saturating_mul(100) >= capacity.saturating_mul(threshold as usize)
}

fn percent_at_least_u64(value: u64, capacity: u64, threshold: u8) -> bool {
    capacity > 0 && value.saturating_mul(100) >= capacity.saturating_mul(threshold as u64)
}

fn strongest_pressure(
    left: Option<AutoscalePressure>,
    right: Option<AutoscalePressure>,
) -> Option<AutoscalePressure> {
    match (left, right) {
        (None, pressure) | (pressure, None) => pressure,
        (Some(left), Some(right)) => Some(if pressure_priority(left) >= pressure_priority(right) {
            left
        } else {
            right
        }),
    }
}

fn pressure_priority(pressure: AutoscalePressure) -> u8 {
    match pressure {
        AutoscalePressure::Memory => 4,
        AutoscalePressure::TickTime => 3,
        AutoscalePressure::FirstChunkSla => 2,
        AutoscalePressure::ChunkQueue => 1,
    }
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
            queued_chunks: 1,
            queue_capacity: 64,
            memory_used_mb: 512,
            memory_limit_mb: 4096,
            first_chunk_ms: Some(500),
        }
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
            "pressure persisted for 2 ticks; applying bounded degradation"
        );

        let cooldown = controller.observe(input);
        assert_eq!(cooldown.action, AutoscaleAction::Hold);
        assert_eq!(cooldown.limits, second.limits);
    }

    #[test]
    fn first_runtime_pressure_immediately_yields_random_tick_work() {
        let mut controller = balanced_controller();
        let pressure = RuntimeControlInput {
            queued_chunks: 64,
            queue_capacity: 64,
            ..healthy_input()
        };

        let decision = controller.observe(pressure);

        assert_eq!(decision.action, AutoscaleAction::Hold);
        assert_eq!(decision.pressure, Some(AutoscalePressure::ChunkQueue));
        assert_eq!(controller.snapshot().work_budgets.random_tick_chunks, 32);
        assert_eq!(controller.snapshot().work_budgets.scheduled_ticks, 256);
    }

    #[test]
    fn healthy_ticks_restore_throughput_without_overshooting_bounds() {
        let mut controller = balanced_controller();
        let pressure = RuntimeControlInput {
            queued_chunks: 64,
            queue_capacity: 64,
            ..healthy_input()
        };
        controller.observe(pressure);
        controller.observe(pressure);

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
            "healthy for 3 ticks; restoring bounded throughput"
        );

        let cooldown = controller.observe(healthy_input());
        assert_eq!(cooldown.action, AutoscaleAction::Hold);
        assert_eq!(cooldown.limits, restored.limits);
    }

    #[test]
    fn pressure_observers_cannot_vote_for_recovery() {
        let mut controller = balanced_controller();
        let slow_tick = RuntimeControlInput {
            tick_ms: 80,
            ..healthy_input()
        };
        controller.observe(slow_tick);
        let scaled_down = controller.observe(slow_tick);

        for _ in 0..4 {
            let decision = controller.report_pressure(healthy_input());
            assert_eq!(decision.action, AutoscaleAction::Hold);
            assert_eq!(decision.pressure, None);
            assert_eq!(decision.limits, scaled_down.limits);
        }

        let snapshot = controller.snapshot();
        assert_eq!(snapshot.healthy_ticks, 0);
        assert_eq!(snapshot.limits, scaled_down.limits);
        assert_eq!(
            snapshot.last_decision.pressure,
            Some(AutoscalePressure::TickTime)
        );
    }

    #[test]
    fn background_pressure_reports_are_coalesced_until_tick_owner_observes() {
        let mut controller = balanced_controller();
        let queue_pressure = RuntimeControlInput {
            queued_chunks: 64,
            queue_capacity: 64,
            ..healthy_input()
        };

        for _ in 0..16 {
            let report = controller.report_pressure(queue_pressure);
            assert_eq!(report.action, AutoscaleAction::Hold);
            assert_eq!(report.pressure, Some(AutoscalePressure::ChunkQueue));
        }

        let before_owner = controller.snapshot();
        assert_eq!(before_owner.pressure_ticks, 0);
        assert_eq!(before_owner.limits.view_distance, 8);

        let first_tick = controller.observe(healthy_input());
        assert_eq!(first_tick.action, AutoscaleAction::Hold);
        assert_eq!(first_tick.pressure, Some(AutoscalePressure::ChunkQueue));
        assert_eq!(controller.snapshot().pressure_ticks, 1);

        controller.report_pressure(queue_pressure);
        let second_tick = controller.observe(healthy_input());
        assert_eq!(second_tick.action, AutoscaleAction::ScaleDown);
        assert_eq!(second_tick.pressure, Some(AutoscalePressure::ChunkQueue));
        assert_eq!(second_tick.limits.view_distance, 7);
    }

    #[test]
    fn memory_pressure_takes_priority_over_a_full_chunk_queue() {
        let mut controller = balanced_controller();

        let decision = controller.observe(RuntimeControlInput {
            queued_chunks: 64,
            queue_capacity: 64,
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
