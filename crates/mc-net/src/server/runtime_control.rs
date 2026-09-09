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
    if decision.action == crate::AutoscaleAction::Hold {
        // Hold is the per-tick steady state. Avoid a synchronous regional-owner
        // command that would invalidate read routes without changing capacity.
        if decision.pressure == Some(crate::AutoscalePressure::Memory) {
            let removed = sessions.shed_prepared_chunks();
            if removed > 0 {
                debug!(removed, "memory pressure released shared prepared chunks");
            }
        }
        return Ok(());
    }
    // Chunk pressure includes requests still waiting for CPU admission. Reduce
    // producer rates through the controller limits, not their drain capacity.
    let cpu_action = if decision.action == crate::AutoscaleAction::ScaleDown
        && decision.pressure == Some(crate::AutoscalePressure::ChunkQueue)
    {
        crate::AutoscaleAction::Hold
    } else {
        decision.action
    };
    let prepare_limit = resources.apply_runtime_control_action(cpu_action, draining);
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
