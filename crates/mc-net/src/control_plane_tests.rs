use super::tests::{balanced_controller, healthy_input};
use super::*;

fn random_work_input() -> RuntimeWorkInput {
    RuntimeWorkInput {
        tick_p95_us: 60_000,
        entity_goals_p95_us: 1_000,
        entity_physics_p95_us: 1_000,
        entity_dispatch_p95_us: 1_000,
        random_tick_p95_us: 18_000,
        block_tick_p95_us: 2_000,
        fluid_tick_p95_us: 1_000,
        scheduled_budget_exhausted: false,
    }
}

#[tokio::test(start_paused = true)]
async fn sustained_pressure_requires_more_than_minute_per_gradual_step() {
    let mut controller = balanced_controller();
    let initial = controller.snapshot();
    let input = RuntimeControlInput {
        tick_ms: 80,
        ..healthy_input()
    };
    for _ in 0..1_000 {
        assert_eq!(controller.observe(input).action, AutoscaleAction::Hold);
    }
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(controller.observe(input).limits, initial.limits);
    tokio::time::advance(Duration::from_nanos(1)).await;
    let first = controller.observe(input);
    assert_eq!(first.action, AutoscaleAction::ScaleDown);
    assert_eq!(
        first.limits,
        RuntimeControlLimits {
            view_distance: 7,
            chunk_send_rate: 15,
            chunk_load_rate: 31,
            chunk_generate_rate: 15,
        }
    );
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(controller.observe(input).limits, first.limits);
    tokio::time::advance(Duration::from_nanos(1)).await;
    let second = controller.observe(input);
    assert_eq!(second.action, AutoscaleAction::ScaleDown);
    assert_eq!(
        second.limits,
        RuntimeControlLimits {
            view_distance: 6,
            chunk_send_rate: 14,
            chunk_load_rate: 30,
            chunk_generate_rate: 14,
        }
    );
}

#[tokio::test(start_paused = true)]
async fn interrupted_pressure_and_signal_bursts_restart_the_minute() {
    let mut controller = balanced_controller();
    let initial = controller.snapshot();
    controller.observe(healthy_input());
    for _ in 0..100 {
        controller.observe_signal(RuntimeControlSignal::ChunkPressure {
            saturated_sources: 1,
        });
        controller.observe_signal(RuntimeControlSignal::ChunkPressure {
            saturated_sources: 0,
        });
    }
    let pressure = RuntimeControlInput {
        tick_ms: 80,
        ..healthy_input()
    };
    controller.observe(pressure);
    tokio::time::advance(Duration::from_secs(59)).await;
    controller.observe(healthy_input());
    controller.observe(pressure);
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(controller.observe(pressure).limits, initial.limits);
    assert_eq!(controller.snapshot().work_budgets, initial.work_budgets);
    tokio::time::advance(Duration::from_nanos(1)).await;
    assert_eq!(
        controller.observe(pressure).action,
        AutoscaleAction::ScaleDown
    );
}

#[tokio::test(start_paused = true)]
async fn coalesced_producer_recovery_restarts_limits_and_work_windows() {
    for sources in 0..3 {
        let mut controller = balanced_controller();
        controller.observe(healthy_input());
        controller.observe_work(RuntimeWorkInput {
            tick_p95_us: 1_000,
            ..random_work_input()
        });
        let initial = controller.snapshot();
        let (producer, mut receiver) = runtime_control_signal_channel();
        let mut queue = producer.chunk_pressure_source();
        let mut sla = producer.first_chunk_sla_source();
        queue.set_saturated(sources != 1);
        sla.set_active(sources != 0);
        while let Some(signal) = receiver.try_recv() {
            controller.observe_signal(signal);
        }
        tokio::time::advance(Duration::from_secs(59)).await;
        queue.set_saturated(false);
        sla.set_active(false);
        queue.set_saturated(sources != 1);
        sla.set_active(sources != 0);
        while let Some(signal) = receiver.try_recv() {
            controller.observe_signal(signal);
        }
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(
            controller.observe(healthy_input()).action,
            AutoscaleAction::Hold
        );
        assert_eq!(controller.snapshot().work_budgets, initial.work_budgets);
        tokio::time::advance(Duration::from_secs(58)).await;
        assert_eq!(controller.observe(healthy_input()).limits, initial.limits);
        tokio::time::advance(Duration::from_nanos(1)).await;
        assert_eq!(
            controller.observe(healthy_input()).action,
            AutoscaleAction::ScaleDown
        );
        assert_eq!(
            controller.snapshot().work_budgets.random_tick_chunks,
            initial.work_budgets.random_tick_chunks - 1
        );
    }
}

#[tokio::test(start_paused = true)]
async fn stable_recovery_requires_headroom_and_a_fresh_minute_per_step() {
    let mut controller = balanced_controller();
    let pressure = RuntimeControlInput {
        tick_ms: 80,
        ..healthy_input()
    };
    controller.observe(pressure);
    tokio::time::advance(Duration::from_secs(61)).await;
    controller.observe(pressure);
    tokio::time::advance(Duration::from_secs(61)).await;
    let reduced = controller.observe(pressure);
    controller.observe(healthy_input());
    tokio::time::advance(Duration::from_secs(59)).await;
    let deadband = RuntimeControlInput {
        tick_ms: 45,
        ..healthy_input()
    };
    controller.observe(deadband);
    tokio::time::advance(Duration::from_secs(120)).await;
    assert_eq!(controller.observe(deadband).limits, reduced.limits);
    controller.observe(healthy_input());
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(controller.observe(healthy_input()).limits, reduced.limits);
    tokio::time::advance(Duration::from_nanos(1)).await;
    let restored = controller.observe(healthy_input());
    assert_eq!(restored.action, AutoscaleAction::ScaleUp);
    assert_eq!(
        restored.limits,
        RuntimeControlLimits {
            view_distance: 7,
            chunk_send_rate: 15,
            chunk_load_rate: 31,
            chunk_generate_rate: 15,
        }
    );
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(controller.observe(healthy_input()).limits, restored.limits);
    tokio::time::advance(Duration::from_nanos(1)).await;
    assert_eq!(controller.observe(healthy_input()).limits.view_distance, 8);
}

#[tokio::test(start_paused = true)]
async fn producer_recovery_preserves_latest_memory_and_tick_pressure() {
    for input in [
        RuntimeControlInput {
            tick_ms: 80,
            ..healthy_input()
        },
        RuntimeControlInput {
            memory_used_mb: 4_000,
            ..healthy_input()
        },
    ] {
        let mut controller = balanced_controller();
        controller.observe(input);
        controller.observe_signal(RuntimeControlSignal::ChunkPressure {
            saturated_sources: 1,
        });
        tokio::time::advance(Duration::from_secs(60)).await;
        let recovered = controller.observe_signal(RuntimeControlSignal::ChunkPressure {
            saturated_sources: 0,
        });
        assert!(recovered.pressure.is_some());
        assert_eq!(recovered.action, AutoscaleAction::Hold);
        tokio::time::advance(Duration::from_nanos(1)).await;
        assert_eq!(controller.observe(input).action, AutoscaleAction::ScaleDown);
    }
}

#[tokio::test(start_paused = true)]
async fn isolated_slow_clients_never_prove_continuous_overload() {
    let mut controller = balanced_controller();
    let initial = controller.snapshot();
    controller.observe_signal(RuntimeControlSignal::SlowClientShed);
    tokio::time::advance(Duration::from_secs(120)).await;
    assert_eq!(
        controller
            .observe_signal(RuntimeControlSignal::SlowClientShed)
            .action,
        AutoscaleAction::Hold
    );
    assert_eq!(controller.snapshot().limits, initial.limits);
    assert_eq!(controller.snapshot().work_budgets, initial.work_budgets);
    let pressure = RuntimeControlInput {
        tick_ms: 80,
        ..healthy_input()
    };
    assert_eq!(controller.observe(pressure).action, AutoscaleAction::Hold);
}

#[tokio::test(start_paused = true)]
async fn work_budgets_require_contiguous_minutes_and_gradual_recovery() {
    let mut controller = balanced_controller();
    let pressure = random_work_input();
    let healthy = RuntimeWorkInput {
        tick_p95_us: 30_000,
        ..pressure
    };
    let initial = controller.snapshot().work_budgets;
    for _ in 0..1_000 {
        assert_eq!(controller.observe_work(pressure).budgets, initial);
    }
    tokio::time::advance(Duration::from_secs(59)).await;
    controller.observe_work(healthy);
    controller.observe_work(pressure);
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(controller.observe_work(pressure).budgets, initial);
    tokio::time::advance(Duration::from_nanos(1)).await;
    let reduced = controller.observe_work(pressure);
    assert_eq!(reduced.action, AutoscaleAction::ScaleDown);
    assert_eq!(reduced.budgets.random_tick_chunks, 63);
    assert_eq!(reduced.budgets.scheduled_ticks, 256);
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(controller.observe_work(pressure).budgets, reduced.budgets);
    tokio::time::advance(Duration::from_nanos(1)).await;
    assert_eq!(
        controller.observe_work(pressure).budgets.random_tick_chunks,
        62
    );
    controller.observe_work(healthy);
    tokio::time::advance(Duration::from_secs(59)).await;
    let deadband = RuntimeWorkInput {
        tick_p95_us: 45_000,
        ..healthy
    };
    controller.observe_work(deadband);
    tokio::time::advance(Duration::from_secs(120)).await;
    assert_eq!(
        controller.observe_work(deadband).budgets.random_tick_chunks,
        62
    );
    controller.observe_work(healthy);
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(
        controller.observe_work(healthy).budgets.random_tick_chunks,
        62
    );
    tokio::time::advance(Duration::from_nanos(1)).await;
    assert_eq!(
        controller.observe_work(healthy).budgets.random_tick_chunks,
        63
    );
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(
        controller.observe_work(healthy).budgets.random_tick_chunks,
        63
    );
    tokio::time::advance(Duration::from_nanos(1)).await;
    assert_eq!(controller.observe_work(healthy).budgets, initial);
}

#[tokio::test(start_paused = true)]
async fn runtime_pressure_cannot_bypass_work_gate_or_allow_early_recovery() {
    let mut controller = balanced_controller();
    let work = RuntimeWorkInput {
        tick_p95_us: 30_000,
        ..random_work_input()
    };
    let initial = controller.snapshot().work_budgets;
    controller.observe_work(work);
    controller.observe(healthy_input());
    controller.observe_signal(RuntimeControlSignal::ChunkPressure {
        saturated_sources: 1,
    });
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(controller.observe_work(work).budgets, initial);
    tokio::time::advance(Duration::from_nanos(1)).await;
    assert_eq!(controller.observe_work(work).budgets.random_tick_chunks, 63);
    controller.observe(RuntimeControlInput {
        memory_used_mb: 4_000,
        ..healthy_input()
    });
    controller.observe_signal(RuntimeControlSignal::ChunkPressure {
        saturated_sources: 0,
    });
    tokio::time::advance(Duration::from_secs(61)).await;
    assert_eq!(controller.observe_work(work).budgets.random_tick_chunks, 62);
    controller.observe(healthy_input());
    tokio::time::advance(Duration::from_secs(60)).await;
    assert_eq!(controller.observe_work(work).budgets.random_tick_chunks, 62);
    tokio::time::advance(Duration::from_nanos(1)).await;
    assert_eq!(controller.observe_work(work).budgets.random_tick_chunks, 63);
}

#[tokio::test(start_paused = true)]
async fn coalesced_source_handoff_preserves_continuous_overload() {
    for starts_with_sla in [true, false] {
        let mut controller = balanced_controller();
        controller.observe(healthy_input());
        let (producer, mut receiver) = runtime_control_signal_channel();
        let mut queue = producer.chunk_pressure_source();
        let mut sla = producer.first_chunk_sla_source();
        queue.set_saturated(!starts_with_sla);
        sla.set_active(starts_with_sla);
        while let Some(signal) = receiver.try_recv() {
            controller.observe_signal(signal);
        }
        tokio::time::advance(Duration::from_secs(59)).await;
        if starts_with_sla {
            queue.set_saturated(true);
            sla.set_active(false);
        } else {
            sla.set_active(true);
            queue.set_saturated(false);
        }
        while let Some(signal) = receiver.try_recv() {
            controller.observe_signal(signal);
        }
        tokio::time::advance(Duration::from_secs(2)).await;
        assert_eq!(
            controller.observe(healthy_input()).action,
            AutoscaleAction::ScaleDown
        );
    }
}
