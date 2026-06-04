//! Bounded runtime-control decisions for chunk throughput and view budgets.
//!
//! This module is deliberately local-process only. It makes per-instance
//! backpressure decisions observable; it does not coordinate shared-world
//! horizontal sharding.

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
    pub worker_pressure_percent: u8,
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
                worker_pressure_percent: 80,
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
                worker_pressure_percent: 85,
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
                worker_pressure_percent: 90,
                memory_pressure_percent: 90,
                scale_down_after_ticks: 4,
                scale_up_after_ticks: 12,
            },
        }
    }

    #[must_use]
    pub fn normalized(self) -> Self {
        Self {
            profile: self.profile,
            min_view_distance: self.min_view_distance.max(2),
            max_view_distance: self.max_view_distance.max(self.min_view_distance.max(2)),
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
            worker_pressure_percent: self.worker_pressure_percent.clamp(1, 100),
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
pub struct RuntimeControlInput {
    pub tick_ms: u64,
    pub queued_chunks: usize,
    pub queue_capacity: usize,
    pub active_workers: usize,
    pub worker_capacity: usize,
    pub memory_used_mb: u64,
    pub memory_limit_mb: u64,
    pub first_chunk_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoscalePressure {
    TickTime,
    ChunkQueue,
    WorkerSaturation,
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
    pub pressure_ticks: u32,
    pub healthy_ticks: u32,
    pub draining: bool,
}

#[derive(Debug, Clone)]
pub struct RuntimeControlPlane {
    policy: AutoscalePolicy,
    limits: RuntimeControlLimits,
    last_decision: AutoscaleDecision,
    pressure_ticks: u32,
    healthy_ticks: u32,
    draining: bool,
}

impl RuntimeControlPlane {
    #[must_use]
    pub fn new(policy: AutoscalePolicy, initial_limits: RuntimeControlLimits) -> Self {
        let policy = policy.normalized();
        let limits = initial_limits.bounded(policy);
        Self {
            policy,
            limits,
            last_decision: AutoscaleDecision {
                action: AutoscaleAction::Hold,
                pressure: None,
                limits,
                reason: "initialized within bounded profile limits".to_string(),
            },
            pressure_ticks: 0,
            healthy_ticks: 0,
            draining: false,
        }
    }

    pub fn request_drain(&mut self) -> AutoscaleDecision {
        self.draining = true;
        self.limits = RuntimeControlLimits {
            view_distance: self.policy.min_view_distance,
            chunk_send_rate: self.policy.min_chunk_send_rate,
            chunk_load_rate: self.policy.min_chunk_load_rate,
            chunk_generate_rate: self.policy.min_chunk_generate_rate,
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

        let pressure = self.pressure(input);
        if let Some(kind) = pressure {
            self.pressure_ticks = self.pressure_ticks.saturating_add(1);
            self.healthy_ticks = 0;
            if self.pressure_ticks >= self.policy.scale_down_after_ticks {
                let before = self.limits;
                self.limits = self.scale_down();
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
                        self.pressure_ticks
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
                    self.healthy_ticks
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

    #[must_use]
    pub fn snapshot(&self) -> RuntimeControlSnapshot {
        RuntimeControlSnapshot {
            policy: self.policy,
            limits: self.limits,
            last_decision: self.last_decision.clone(),
            pressure_ticks: self.pressure_ticks,
            healthy_ticks: self.healthy_ticks,
            draining: self.draining,
        }
    }

    fn record(&mut self, decision: AutoscaleDecision) -> AutoscaleDecision {
        self.last_decision = decision.clone();
        decision
    }

    fn pressure(&self, input: RuntimeControlInput) -> Option<AutoscalePressure> {
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
        if percent_at_least(
            input.active_workers,
            input.worker_capacity,
            self.policy.worker_pressure_percent,
        ) {
            return Some(AutoscalePressure::WorkerSaturation);
        }
        if percent_at_least_u64(
            input.memory_used_mb,
            input.memory_limit_mb,
            self.policy.memory_pressure_percent,
        ) {
            return Some(AutoscalePressure::Memory);
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

fn halve_floor(value: u32, floor: u32) -> u32 {
    value.saturating_sub(value / 2).max(floor)
}

fn double_ceiling(value: u32, ceiling: u32) -> u32 {
    value.saturating_mul(2).min(ceiling)
}

fn percent_at_least(value: usize, capacity: usize, threshold: u8) -> bool {
    capacity > 0 && value.saturating_mul(100) >= capacity.saturating_mul(threshold as usize)
}

fn percent_at_least_u64(value: u64, capacity: u64, threshold: u8) -> bool {
    capacity > 0 && value.saturating_mul(100) >= capacity.saturating_mul(threshold as u64)
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
            active_workers: 1,
            worker_capacity: 4,
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
        assert!(second.reason.contains("pressure persisted"));

        let cooldown = controller.observe(input);
        assert_eq!(cooldown.action, AutoscaleAction::Hold);
        assert_eq!(cooldown.limits, second.limits);
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

        let cooldown = controller.observe(healthy_input());
        assert_eq!(cooldown.action, AutoscaleAction::Hold);
        assert_eq!(cooldown.limits, restored.limits);
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
    fn policies_normalize_invalid_bounds() {
        let policy = AutoscalePolicy {
            max_view_distance: 1,
            min_view_distance: 0,
            max_chunk_send_rate: 0,
            min_chunk_send_rate: 0,
            queue_pressure_percent: 0,
            worker_pressure_percent: 250,
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
        assert_eq!(policy.worker_pressure_percent, 100);
        assert_eq!(policy.memory_pressure_percent, 1);
        assert_eq!(policy.scale_down_after_ticks, 1);
        assert_eq!(policy.scale_up_after_ticks, 1);
    }
}
