use tracing::{debug, info};

use crate::chunk_pipeline::ChunkPipelineResources;
use crate::control_plane::RuntimeControlApplyError;
use crate::play;

pub(super) fn apply_runtime_control_decision(
    resources: &ChunkPipelineResources,
    sessions: &play::SessionRegistry,
    decision: &crate::AutoscaleDecision,
    draining: bool,
) -> Result<(), RuntimeControlApplyError> {
    let previous_prepare_limit = resources.prepare_limit();
    // Chunk pressure includes requests still waiting for CPU admission. The
    // resources layer owns the physical response: it holds background drain
    // capacity under chunk pressure and grows the shared pools toward the
    // policy ceilings while one worker demonstrably cannot keep up. Holds
    // still apply so fully-idle adaptive pools can shrink back toward one
    // worker.
    let prepare_limit =
        resources.apply_runtime_control_action(decision.action, decision.pressure, draining);
    if draining {
        let entity_owner_lanes = sessions.reconfigure_entity_owner_lanes(1);
        if entity_owner_lanes != 1 {
            return Err(RuntimeControlApplyError::controlled_stop(format!(
                "runtime drain requires one entity-owner lane but authority applied {entity_owner_lanes}"
            )));
        }
    }
    if prepare_limit != previous_prepare_limit {
        info!(
            action = ?decision.action,
            pressure = ?decision.pressure,
            prepare_limit,
            reason = %decision.reason,
            "runtime background chunk preparation admission changed"
        );
    }
    if decision.pressure == Some(crate::AutoscalePressure::Memory) {
        let removed = sessions.shed_prepared_chunks();
        if removed > 0 {
            debug!(removed, "memory pressure released shared prepared chunks");
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "runtime_control_tests.rs"]
mod tests;
