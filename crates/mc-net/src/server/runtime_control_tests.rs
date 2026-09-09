use super::*;

#[test]
fn chunk_queue_backpressure_preserves_capacity_to_drain_work() {
    let resources = ChunkPipelineResources::with_limits(1, 9);
    let sessions = play::SessionRegistry::new();
    let mut decision = crate::AutoscaleDecision {
        action: crate::AutoscaleAction::ScaleDown,
        pressure: Some(crate::AutoscalePressure::ChunkQueue),
        limits: crate::RuntimeControlLimits {
            view_distance: 6,
            chunk_send_rate: 8,
            chunk_load_rate: 16,
            chunk_generate_rate: 8,
        },
        reason: "queued chunk work needs backpressure".to_string(),
    };
    apply_runtime_control_decision(&resources, &sessions, &decision, false).unwrap();
    let permits: Vec<_> = (0..8)
        .map(|_| {
            resources
                .try_acquire_prepare_request()
                .expect("queued work retains its preparation capacity")
        })
        .collect();
    assert!(
        resources.try_acquire_prepare_request().is_none(),
        "capacity remains bounded"
    );
    drop(permits);

    decision.pressure = Some(crate::AutoscalePressure::TickTime);
    apply_runtime_control_decision(&resources, &sessions, &decision, false).unwrap();
    let permits: Vec<_> = (0..4)
        .map(|_| {
            resources
                .try_acquire_prepare_request()
                .expect("reduced capacity remains usable")
        })
        .collect();
    assert!(
        resources.try_acquire_prepare_request().is_none(),
        "slow ticks still reduce background preparation admission"
    );
    drop(permits);
}

#[test]
fn draining_overrides_scale_up_without_cancelling_admitted_preparation() {
    let resources = ChunkPipelineResources::with_limits(1, 3);
    let sessions = play::SessionRegistry::new_with_entity_owner_lanes(3);
    let first = resources.try_acquire_prepare_request().unwrap();
    let second = resources.try_acquire_prepare_request().unwrap();
    let decision = crate::AutoscaleDecision {
        action: crate::AutoscaleAction::ScaleUp,
        pressure: None,
        limits: crate::RuntimeControlLimits {
            view_distance: 6,
            chunk_send_rate: 8,
            chunk_load_rate: 16,
            chunk_generate_rate: 8,
        },
        reason: "drain overrides recovery".to_string(),
    };
    apply_runtime_control_decision(&resources, &sessions, &decision, true).unwrap();
    assert_eq!(sessions.entity_owner_lane_count(), 1);
    assert!(resources.try_acquire_prepare_request().is_none());
    drop(first);
    assert!(resources.try_acquire_prepare_request().is_none());
    drop(second);
    let remaining = resources.try_acquire_prepare_request().unwrap();
    assert!(resources.try_acquire_prepare_request().is_none());
    drop(remaining);
}
