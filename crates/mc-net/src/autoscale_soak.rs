use crate::{
    AutoscaleAction, ChunkPipelinePolicy, ChunkPipelineResourceSnapshot, ChunkPipelineStopReason,
    OutboundPressureSnapshot, RuntimeControlSnapshot, SaveAllReport,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoscaleSoakProfile {
    LowEnd,
    Balanced,
    HighEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoscaleSoakScenario {
    SlowDisk,
    ChunkGenerationStorm,
    ReconnectStorm,
    SlowClient,
    SaveDuringShutdown,
    DrainRestart,
    MemoryPressure,
    QueueSaturation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoscalePrimitiveStatus {
    Present,
    Degraded { reason: &'static str },
}

#[derive(Debug, Clone)]
pub struct AutoscaleSoakSnapshot<'a> {
    pub profile: AutoscaleSoakProfile,
    pub scenarios: &'a [AutoscaleSoakScenario],
    pub chunk_policy: ChunkPipelinePolicy,
    pub chunk_resources: ChunkPipelineResourceSnapshot,
    pub chunk_stop_reasons: &'a [ChunkPipelineStopReason],
    pub outbound_pressure: OutboundPressureSnapshot,
    pub save_all: Option<&'a SaveAllReport>,
    pub memory_pressure_shed_chunks: usize,
    pub runtime_control: Option<&'a RuntimeControlSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoscaleSoakReport {
    pub profile: AutoscaleSoakProfile,
    pub scenarios_attempted: Vec<AutoscaleSoakScenario>,
    pub dynamic_autoscale: AutoscalePrimitiveStatus,
    pub bounded_chunk_queue: AutoscalePrimitiveStatus,
    pub worker_backpressure: AutoscalePrimitiveStatus,
    pub slow_client_pressure: AutoscalePrimitiveStatus,
    pub memory_pressure_shedding: AutoscalePrimitiveStatus,
    pub save_recovery_visibility: AutoscalePrimitiveStatus,
    pub queue_saturation_observed: bool,
    pub slow_client_pressure_observed: bool,
    pub memory_pressure_shedding_observed: bool,
    pub memory_pressure_shed_chunks: usize,
    pub save_errors_observed: usize,
    pub gaps: Vec<&'static str>,
}

impl AutoscaleSoakReport {
    #[must_use]
    pub fn from_snapshot(snapshot: AutoscaleSoakSnapshot<'_>) -> Self {
        let queue_saturation_attempted = snapshot
            .scenarios
            .contains(&AutoscaleSoakScenario::QueueSaturation);
        let slow_client_attempted = snapshot
            .scenarios
            .contains(&AutoscaleSoakScenario::SlowClient);
        let memory_pressure_attempted = snapshot
            .scenarios
            .contains(&AutoscaleSoakScenario::MemoryPressure);
        let save_during_shutdown_attempted = snapshot
            .scenarios
            .contains(&AutoscaleSoakScenario::SaveDuringShutdown);
        let queue_saturation_observed = queue_saturation_attempted
            && snapshot
                .chunk_stop_reasons
                .contains(&ChunkPipelineStopReason::QueueFull);
        let slow_client_pressure_observed = slow_client_attempted
            && (snapshot.outbound_pressure.best_effort_animation_drops > 0
                || snapshot.outbound_pressure.reliable_command_drops > 0
                || snapshot.outbound_pressure.reliable_command_retries > 0
                || snapshot
                    .outbound_pressure
                    .reliable_command_retries_in_flight
                    > 0
                || snapshot.outbound_pressure.slow_client_write_timeouts > 0
                || snapshot.outbound_pressure.slow_client_pressure_sheds > 0);
        let memory_pressure_shedding_observed = memory_pressure_attempted
            && snapshot.memory_pressure_shed_chunks > 0
            && snapshot
                .chunk_stop_reasons
                .contains(&ChunkPipelineStopReason::MemoryPressure);
        let dynamic_autoscale_observed = snapshot.runtime_control.is_some_and(|runtime| {
            !runtime.draining
                && (runtime.last_decision.pressure.is_some()
                    || !matches!(runtime.last_decision.action, AutoscaleAction::Hold))
        });
        let save_errors_observed = snapshot.save_all.map_or(0, |report| report.errors.len());

        let mut gaps = vec![
            "profile validation is config-level only, not measured against low-end/balanced/high-end hardware",
            "memory-pressure recovery and soak evidence are absent",
        ];
        if !dynamic_autoscale_observed {
            gaps.push("dynamic runtime-control pressure/action decision was not observed");
        }
        if !memory_pressure_shedding_observed {
            gaps.push("memory-pressure shedding evidence is absent");
        }

        for (scenario, gap) in [
            (
                AutoscaleSoakScenario::SlowDisk,
                "slow-disk recovery scenario not run",
            ),
            (
                AutoscaleSoakScenario::ChunkGenerationStorm,
                "chunk-generation storm scenario not run",
            ),
            (
                AutoscaleSoakScenario::ReconnectStorm,
                "reconnect-storm scenario not run",
            ),
            (
                AutoscaleSoakScenario::SlowClient,
                "slow-client scenario not run",
            ),
            (
                AutoscaleSoakScenario::SaveDuringShutdown,
                "save-during-shutdown scenario not run",
            ),
            (
                AutoscaleSoakScenario::DrainRestart,
                "drain/restart scenario not run",
            ),
            (
                AutoscaleSoakScenario::MemoryPressure,
                "memory-pressure scenario not run",
            ),
            (
                AutoscaleSoakScenario::QueueSaturation,
                "queue-saturation scenario not run",
            ),
        ] {
            if !snapshot.scenarios.contains(&scenario) {
                gaps.push(gap);
            }
        }

        Self {
            profile: snapshot.profile,
            scenarios_attempted: snapshot.scenarios.to_vec(),
            dynamic_autoscale: if dynamic_autoscale_observed {
                AutoscalePrimitiveStatus::Present
            } else {
                AutoscalePrimitiveStatus::Degraded {
                    reason: "no runtime-control pressure/action decision was observed in this bounded slice",
                }
            },
            bounded_chunk_queue: if snapshot.chunk_policy.chunk_result_queue_size == 0 {
                AutoscalePrimitiveStatus::Degraded {
                    reason: "chunk result queue bound is zero after policy construction",
                }
            } else if snapshot.chunk_resources.max_result_queue_depth == 0 {
                AutoscalePrimitiveStatus::Degraded {
                    reason: "chunk result queue was not exercised in this bounded slice",
                }
            } else if snapshot.chunk_resources.max_result_queue_depth
                <= snapshot.chunk_policy.chunk_result_queue_size
            {
                AutoscalePrimitiveStatus::Present
            } else {
                AutoscalePrimitiveStatus::Degraded {
                    reason: "observed chunk result queue depth exceeded configured bound",
                }
            },
            worker_backpressure: if snapshot.chunk_resources.max_io_active == 0
                || snapshot.chunk_resources.max_cpu_active == 0
            {
                AutoscalePrimitiveStatus::Degraded {
                    reason: "worker limits were not exercised in this bounded slice",
                }
            } else if snapshot.chunk_resources.max_io_active
                <= snapshot.chunk_policy.chunk_io_threads
                && snapshot.chunk_resources.max_cpu_active
                    <= snapshot.chunk_policy.chunk_worker_threads
            {
                AutoscalePrimitiveStatus::Present
            } else {
                AutoscalePrimitiveStatus::Degraded {
                    reason: "observed worker concurrency exceeded configured permits",
                }
            },
            slow_client_pressure: if slow_client_pressure_observed {
                AutoscalePrimitiveStatus::Present
            } else {
                AutoscalePrimitiveStatus::Degraded {
                    reason: "no slow-client outbound pressure was observed in this bounded slice",
                }
            },
            memory_pressure_shedding: if memory_pressure_shedding_observed {
                AutoscalePrimitiveStatus::Present
            } else {
                AutoscalePrimitiveStatus::Degraded {
                    reason: "no memory-pressure work shedding was observed in this bounded slice",
                }
            },
            save_recovery_visibility: match (save_during_shutdown_attempted, snapshot.save_all) {
                (true, Some(report)) if report.is_ok() => AutoscalePrimitiveStatus::Present,
                (true, Some(_)) => AutoscalePrimitiveStatus::Degraded {
                    reason: "save-all recovery report contained errors",
                },
                (false, _) | (true, None) => AutoscalePrimitiveStatus::Degraded {
                    reason: "save-all recovery report was not captured",
                },
            },
            queue_saturation_observed,
            slow_client_pressure_observed,
            memory_pressure_shedding_observed,
            memory_pressure_shed_chunks: snapshot.memory_pressure_shed_chunks,
            save_errors_observed,
            gaps,
        }
    }

    #[must_use]
    pub fn is_degraded(&self) -> bool {
        !matches!(self.dynamic_autoscale, AutoscalePrimitiveStatus::Present)
            || !matches!(self.bounded_chunk_queue, AutoscalePrimitiveStatus::Present)
            || !matches!(self.worker_backpressure, AutoscalePrimitiveStatus::Present)
            || !matches!(self.slow_client_pressure, AutoscalePrimitiveStatus::Present)
            || !matches!(
                self.memory_pressure_shedding,
                AutoscalePrimitiveStatus::Present
            )
            || !matches!(
                self.save_recovery_visibility,
                AutoscalePrimitiveStatus::Present
            )
            || !self.gaps.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AutoscaleAction, AutoscalePolicy, AutoscalePressure, AutoscaleProfile,
        RuntimeControlLimits, RuntimeControlPlane, SaveAllTimings,
        control_plane::RuntimeControlSignal, server::SaveAllReport,
    };

    fn policy() -> ChunkPipelinePolicy {
        ChunkPipelinePolicy {
            chunk_send_rate: 2,
            chunk_load_rate: 2,
            chunk_generate_rate: 1,
            chunk_prepare_budget_ms: 1,
            chunk_prepare_batch_size: 1,
            chunk_io_threads: 1,
            chunk_worker_threads: 1,
            chunk_result_queue_size: 1,
            region_cache_size: 1,
            compression_threshold: 256,
            compression_level: None,
            runtime_control: None,
        }
    }

    #[test]
    fn bounded_soak_report_marks_absent_dynamic_autoscale_as_degraded() {
        let save = SaveAllReport {
            players_saved: 1,
            entities_saved: 1,
            chunks_flushed: 2,
            world_metadata_saved: true,
            timings: SaveAllTimings::default(),
            errors: Vec::new(),
        };
        let report = AutoscaleSoakReport::from_snapshot(AutoscaleSoakSnapshot {
            profile: AutoscaleSoakProfile::LowEnd,
            scenarios: &[
                AutoscaleSoakScenario::SlowDisk,
                AutoscaleSoakScenario::ChunkGenerationStorm,
                AutoscaleSoakScenario::ReconnectStorm,
                AutoscaleSoakScenario::QueueSaturation,
                AutoscaleSoakScenario::SlowClient,
                AutoscaleSoakScenario::SaveDuringShutdown,
                AutoscaleSoakScenario::DrainRestart,
                AutoscaleSoakScenario::MemoryPressure,
            ],
            chunk_policy: policy(),
            chunk_resources: ChunkPipelineResourceSnapshot {
                active_io: 0,
                max_io_active: 1,
                active_cpu: 0,
                max_cpu_active: 1,
                max_result_queue_depth: 1,
            },
            chunk_stop_reasons: &[ChunkPipelineStopReason::QueueFull],
            outbound_pressure: OutboundPressureSnapshot {
                best_effort_animation_drops: 3,
                reliable_command_drops: 0,
                reliable_command_retries: 1,
                reliable_command_retries_in_flight: 0,
                max_reliable_command_retries_in_flight: 1,
                slow_client_write_timeouts: 0,
                slow_client_pressure_sheds: 0,
            },
            save_all: Some(&save),
            memory_pressure_shed_chunks: 0,
            runtime_control: None,
        });

        assert_eq!(
            report.dynamic_autoscale,
            AutoscalePrimitiveStatus::Degraded {
                reason: "no runtime-control pressure/action decision was observed in this bounded slice"
            }
        );
        assert_eq!(
            report.bounded_chunk_queue,
            AutoscalePrimitiveStatus::Present
        );
        assert_eq!(
            report.worker_backpressure,
            AutoscalePrimitiveStatus::Present
        );
        assert_eq!(
            report.slow_client_pressure,
            AutoscalePrimitiveStatus::Present
        );
        assert_eq!(
            report.save_recovery_visibility,
            AutoscalePrimitiveStatus::Present
        );
        assert!(report.queue_saturation_observed);
        assert!(report.slow_client_pressure_observed);
        assert!(
            report
                .gaps
                .contains(&"dynamic runtime-control pressure/action decision was not observed")
        );
        assert!(report.is_degraded());
    }

    #[test]
    fn bounded_soak_report_marks_observed_runtime_control_pressure_decision_as_present() {
        let mut controller = RuntimeControlPlane::new(
            AutoscalePolicy {
                scale_down_after_ticks: 1,
                ..AutoscalePolicy::for_profile(AutoscaleProfile::Balanced)
            },
            RuntimeControlLimits {
                view_distance: 8,
                chunk_send_rate: 16,
                chunk_load_rate: 32,
                chunk_generate_rate: 16,
            },
        );
        let decision = controller.observe_signal(RuntimeControlSignal::ChunkPressure {
            saturated_sources: 1,
        });
        assert_eq!(decision.action, AutoscaleAction::ScaleDown);
        assert_eq!(decision.pressure, Some(AutoscalePressure::ChunkQueue));
        let runtime_control = controller.snapshot();

        let report = AutoscaleSoakReport::from_snapshot(AutoscaleSoakSnapshot {
            profile: AutoscaleSoakProfile::Balanced,
            scenarios: &[AutoscaleSoakScenario::QueueSaturation],
            chunk_policy: policy(),
            chunk_resources: ChunkPipelineResourceSnapshot {
                active_io: 0,
                max_io_active: 1,
                active_cpu: 0,
                max_cpu_active: 1,
                max_result_queue_depth: 1,
            },
            chunk_stop_reasons: &[ChunkPipelineStopReason::QueueFull],
            outbound_pressure: OutboundPressureSnapshot::default(),
            save_all: None,
            memory_pressure_shed_chunks: 0,
            runtime_control: Some(&runtime_control),
        });

        assert_eq!(report.dynamic_autoscale, AutoscalePrimitiveStatus::Present);
        assert!(report.queue_saturation_observed);
        assert!(report.is_degraded());
        assert!(
            !report
                .gaps
                .contains(&"dynamic runtime-control pressure/action decision was not observed")
        );
    }

    #[test]
    fn bounded_soak_report_keeps_idle_runtime_control_snapshot_degraded() {
        let controller = RuntimeControlPlane::new(
            AutoscalePolicy::for_profile(AutoscaleProfile::Balanced),
            RuntimeControlLimits {
                view_distance: 8,
                chunk_send_rate: 16,
                chunk_load_rate: 32,
                chunk_generate_rate: 16,
            },
        );
        let runtime_control = controller.snapshot();

        let report = AutoscaleSoakReport::from_snapshot(AutoscaleSoakSnapshot {
            profile: AutoscaleSoakProfile::Balanced,
            scenarios: &[AutoscaleSoakScenario::QueueSaturation],
            chunk_policy: policy(),
            chunk_resources: ChunkPipelineResourceSnapshot {
                active_io: 0,
                max_io_active: 1,
                active_cpu: 0,
                max_cpu_active: 1,
                max_result_queue_depth: 1,
            },
            chunk_stop_reasons: &[ChunkPipelineStopReason::QueueFull],
            outbound_pressure: OutboundPressureSnapshot::default(),
            save_all: None,
            memory_pressure_shed_chunks: 0,
            runtime_control: Some(&runtime_control),
        });

        assert_eq!(
            report.dynamic_autoscale,
            AutoscalePrimitiveStatus::Degraded {
                reason: "no runtime-control pressure/action decision was observed in this bounded slice"
            }
        );
        assert!(
            report
                .gaps
                .contains(&"dynamic runtime-control pressure/action decision was not observed")
        );
    }

    #[test]
    fn bounded_soak_report_keeps_drain_runtime_control_snapshot_degraded() {
        let mut controller = RuntimeControlPlane::new(
            AutoscalePolicy::for_profile(AutoscaleProfile::Balanced),
            RuntimeControlLimits {
                view_distance: 8,
                chunk_send_rate: 16,
                chunk_load_rate: 32,
                chunk_generate_rate: 16,
            },
        );
        let drain_decision = controller.request_drain();
        assert_eq!(drain_decision.action, AutoscaleAction::ScaleDown);
        assert_eq!(drain_decision.pressure, None);
        let runtime_control = controller.snapshot();
        assert!(runtime_control.draining);

        let report = AutoscaleSoakReport::from_snapshot(AutoscaleSoakSnapshot {
            profile: AutoscaleSoakProfile::Balanced,
            scenarios: &[
                AutoscaleSoakScenario::DrainRestart,
                AutoscaleSoakScenario::QueueSaturation,
            ],
            chunk_policy: policy(),
            chunk_resources: ChunkPipelineResourceSnapshot {
                active_io: 0,
                max_io_active: 1,
                active_cpu: 0,
                max_cpu_active: 1,
                max_result_queue_depth: 1,
            },
            chunk_stop_reasons: &[ChunkPipelineStopReason::QueueFull],
            outbound_pressure: OutboundPressureSnapshot::default(),
            save_all: None,
            memory_pressure_shed_chunks: 0,
            runtime_control: Some(&runtime_control),
        });

        assert_eq!(
            report.dynamic_autoscale,
            AutoscalePrimitiveStatus::Degraded {
                reason: "no runtime-control pressure/action decision was observed in this bounded slice"
            }
        );
        assert!(
            report
                .gaps
                .contains(&"dynamic runtime-control pressure/action decision was not observed")
        );
    }

    #[test]
    fn bounded_soak_report_records_missing_recovery_evidence() {
        let report = AutoscaleSoakReport::from_snapshot(AutoscaleSoakSnapshot {
            profile: AutoscaleSoakProfile::Balanced,
            scenarios: &[AutoscaleSoakScenario::ChunkGenerationStorm],
            chunk_policy: policy(),
            chunk_resources: ChunkPipelineResourceSnapshot {
                active_io: 0,
                max_io_active: 2,
                active_cpu: 0,
                max_cpu_active: 1,
                max_result_queue_depth: 1,
            },
            chunk_stop_reasons: &[ChunkPipelineStopReason::LoadBudget],
            outbound_pressure: OutboundPressureSnapshot::default(),
            save_all: None,
            memory_pressure_shed_chunks: 0,
            runtime_control: None,
        });

        assert_eq!(
            report.worker_backpressure,
            AutoscalePrimitiveStatus::Degraded {
                reason: "observed worker concurrency exceeded configured permits"
            }
        );
        assert_eq!(
            report.slow_client_pressure,
            AutoscalePrimitiveStatus::Degraded {
                reason: "no slow-client outbound pressure was observed in this bounded slice"
            }
        );
        assert_eq!(
            report.save_recovery_visibility,
            AutoscalePrimitiveStatus::Degraded {
                reason: "save-all recovery report was not captured"
            }
        );
        assert!(report.gaps.contains(&"slow-disk recovery scenario not run"));
        assert!(report.gaps.contains(&"reconnect-storm scenario not run"));
        assert!(report.gaps.contains(&"slow-client scenario not run"));
        assert!(
            report
                .gaps
                .contains(&"save-during-shutdown scenario not run")
        );
        assert!(report.gaps.contains(&"drain/restart scenario not run"));
        assert!(report.gaps.contains(&"memory-pressure scenario not run"));
        assert!(report.gaps.contains(&"queue-saturation scenario not run"));
        assert!(!report.queue_saturation_observed);
        assert!(report.is_degraded());
    }

    #[test]
    fn bounded_soak_report_counts_slow_client_write_timeout_as_pressure() {
        let report = AutoscaleSoakReport::from_snapshot(AutoscaleSoakSnapshot {
            profile: AutoscaleSoakProfile::Balanced,
            scenarios: &[AutoscaleSoakScenario::SlowClient],
            chunk_policy: policy(),
            chunk_resources: ChunkPipelineResourceSnapshot {
                active_io: 0,
                max_io_active: 1,
                active_cpu: 0,
                max_cpu_active: 1,
                max_result_queue_depth: 0,
            },
            chunk_stop_reasons: &[],
            outbound_pressure: OutboundPressureSnapshot {
                slow_client_write_timeouts: 1,
                slow_client_pressure_sheds: 0,
                ..OutboundPressureSnapshot::default()
            },
            save_all: None,
            memory_pressure_shed_chunks: 0,
            runtime_control: None,
        });

        assert_eq!(
            report.slow_client_pressure,
            AutoscalePrimitiveStatus::Present
        );
        assert!(report.slow_client_pressure_observed);
    }

    #[test]
    fn bounded_soak_report_counts_slow_client_pressure_shed_as_pressure() {
        let report = AutoscaleSoakReport::from_snapshot(AutoscaleSoakSnapshot {
            profile: AutoscaleSoakProfile::Balanced,
            scenarios: &[AutoscaleSoakScenario::SlowClient],
            chunk_policy: policy(),
            chunk_resources: ChunkPipelineResourceSnapshot {
                active_io: 0,
                max_io_active: 1,
                active_cpu: 0,
                max_cpu_active: 1,
                max_result_queue_depth: 0,
            },
            chunk_stop_reasons: &[],
            outbound_pressure: OutboundPressureSnapshot {
                slow_client_pressure_sheds: 1,
                ..OutboundPressureSnapshot::default()
            },
            save_all: None,
            memory_pressure_shed_chunks: 0,
            runtime_control: None,
        });

        assert_eq!(
            report.slow_client_pressure,
            AutoscalePrimitiveStatus::Present
        );
        assert!(report.slow_client_pressure_observed);
    }

    #[test]
    fn bounded_soak_report_counts_memory_pressure_shedding_as_focused_evidence() {
        let report = AutoscaleSoakReport::from_snapshot(AutoscaleSoakSnapshot {
            profile: AutoscaleSoakProfile::Balanced,
            scenarios: &[AutoscaleSoakScenario::MemoryPressure],
            chunk_policy: policy(),
            chunk_resources: ChunkPipelineResourceSnapshot {
                active_io: 0,
                max_io_active: 1,
                active_cpu: 0,
                max_cpu_active: 1,
                max_result_queue_depth: 1,
            },
            chunk_stop_reasons: &[ChunkPipelineStopReason::MemoryPressure],
            outbound_pressure: OutboundPressureSnapshot::default(),
            save_all: None,
            memory_pressure_shed_chunks: 3,
            runtime_control: None,
        });

        assert_eq!(
            report.memory_pressure_shedding,
            AutoscalePrimitiveStatus::Present
        );
        assert!(report.memory_pressure_shedding_observed);
        assert_eq!(report.memory_pressure_shed_chunks, 3);
        assert!(
            !report
                .gaps
                .contains(&"memory-pressure shedding evidence is absent")
        );
        assert!(
            report
                .gaps
                .contains(&"memory-pressure recovery and soak evidence are absent")
        );
        assert!(report.is_degraded());
    }

    #[test]
    fn bounded_soak_report_does_not_treat_counters_as_unscoped_scenario_evidence() {
        let save = SaveAllReport {
            players_saved: 1,
            entities_saved: 1,
            chunks_flushed: 2,
            world_metadata_saved: true,
            timings: SaveAllTimings::default(),
            errors: Vec::new(),
        };
        let report = AutoscaleSoakReport::from_snapshot(AutoscaleSoakSnapshot {
            profile: AutoscaleSoakProfile::HighEnd,
            scenarios: &[AutoscaleSoakScenario::ChunkGenerationStorm],
            chunk_policy: policy(),
            chunk_resources: ChunkPipelineResourceSnapshot {
                active_io: 0,
                max_io_active: 1,
                active_cpu: 0,
                max_cpu_active: 1,
                max_result_queue_depth: 1,
            },
            chunk_stop_reasons: &[ChunkPipelineStopReason::QueueFull],
            outbound_pressure: OutboundPressureSnapshot {
                best_effort_animation_drops: 3,
                reliable_command_drops: 0,
                reliable_command_retries: 1,
                reliable_command_retries_in_flight: 1,
                max_reliable_command_retries_in_flight: 1,
                slow_client_write_timeouts: 0,
                slow_client_pressure_sheds: 0,
            },
            save_all: Some(&save),
            memory_pressure_shed_chunks: 0,
            runtime_control: None,
        });

        assert!(!report.queue_saturation_observed);
        assert!(!report.slow_client_pressure_observed);
        assert_eq!(
            report.slow_client_pressure,
            AutoscalePrimitiveStatus::Degraded {
                reason: "no slow-client outbound pressure was observed in this bounded slice"
            }
        );
        assert_eq!(
            report.save_recovery_visibility,
            AutoscalePrimitiveStatus::Degraded {
                reason: "save-all recovery report was not captured"
            }
        );
    }

    #[test]
    fn bounded_soak_report_rejects_unexercised_workers_and_failed_save() {
        let save = SaveAllReport {
            players_saved: 0,
            entities_saved: 0,
            chunks_flushed: 0,
            world_metadata_saved: false,
            timings: SaveAllTimings::default(),
            errors: vec!["disk write failed".to_string()],
        };
        let report = AutoscaleSoakReport::from_snapshot(AutoscaleSoakSnapshot {
            profile: AutoscaleSoakProfile::Balanced,
            scenarios: &[AutoscaleSoakScenario::SaveDuringShutdown],
            chunk_policy: policy(),
            chunk_resources: ChunkPipelineResourceSnapshot {
                active_io: 0,
                max_io_active: 0,
                active_cpu: 0,
                max_cpu_active: 0,
                max_result_queue_depth: 0,
            },
            chunk_stop_reasons: &[],
            outbound_pressure: OutboundPressureSnapshot::default(),
            save_all: Some(&save),
            memory_pressure_shed_chunks: 0,
            runtime_control: None,
        });

        assert_eq!(
            report.bounded_chunk_queue,
            AutoscalePrimitiveStatus::Degraded {
                reason: "chunk result queue was not exercised in this bounded slice"
            }
        );
        assert_eq!(
            report.worker_backpressure,
            AutoscalePrimitiveStatus::Degraded {
                reason: "worker limits were not exercised in this bounded slice"
            }
        );
        assert_eq!(
            report.save_recovery_visibility,
            AutoscalePrimitiveStatus::Degraded {
                reason: "save-all recovery report contained errors"
            }
        );
        assert_eq!(report.save_errors_observed, 1);
    }
}
