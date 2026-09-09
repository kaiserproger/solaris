use super::*;
use crate::lock_metrics::TimedGuard;
use std::ops::DerefMut;
use tokio::sync::MutexGuard;

impl SimulationOwner {
    pub(super) fn process_world_save_barrier(
        &mut self,
        sessions: &SessionRegistry,
        storage: TimedGuard<MutexGuard<'_, WorldStorage>>,
        envelope: SimulationCommandEnvelope,
    ) -> SimulationTickReport {
        #[cfg(test)]
        self.last_region_routes.clear();
        let processed = if let Some(envelope) = self.active_session_envelope(sessions, envelope) {
            #[cfg(feature = "load-bench")]
            let command_started = Instant::now();
            let response = self.save_barrier_response(
                sessions,
                Some(storage),
                SimulationRequestError::WorldUnavailable,
                true,
            );
            #[cfg(feature = "load-bench")]
            self.metrics.record_command_kind(
                envelope.command.kind(),
                u64::try_from(command_started.elapsed().as_micros()).unwrap_or(u64::MAX),
            );
            self.metrics.processed.fetch_add(1, Ordering::Relaxed);
            envelope.respond(Ok(response));
            1
        } else {
            0
        };
        SimulationTickReport {
            processed,
            remaining_depth: self.metrics.depth.load(Ordering::Relaxed),
            ..SimulationTickReport::default()
        }
    }

    pub(super) fn save_barrier_response(
        &self,
        sessions: &SessionRegistry,
        mut storage: Option<impl DerefMut<Target = WorldStorage>>,
        world_error: SimulationRequestError,
        capture_world: bool,
    ) -> SimulationResponse {
        let simulation_tick = sessions.simulation_tick();
        let world_flush_plan = if capture_world {
            match storage.as_deref_mut() {
                Some(storage) => match storage.plan_dirty_flush_at_tick(simulation_tick) {
                    Ok(plan) => Ok(Some(plan)),
                    Err(error) => {
                        self.metrics
                            .rejected_world_mutation
                            .fetch_add(1, Ordering::Relaxed);
                        warn!(%error, "simulation save barrier world plan failed");
                        Err(SimulationRequestError::WorldMutationFailed)
                    }
                },
                None => {
                    self.record_world_access_error(world_error);
                    Err(world_error)
                }
            }
        } else {
            Ok(None)
        };
        SimulationResponse::SaveSnapshot(world_flush_plan.map(|world_flush_plan| {
            let world_chunk_journal_watermark = sessions.world_chunk_journal_watermark();
            // The immutable world plan and its WAL cut no longer need the world mutex.
            // The simulation owner still fences entity/player capture and later commands.
            drop(storage);
            let (entities, entity_journal_phases) = sessions.persisted_entity_save_snapshot();
            Box::new(SimulationSaveSnapshot {
                players: sessions.persisted_player_states(),
                entities,
                entity_journal_phases,
                world_chunk_journal_watermark,
                world_time: sessions.world_time(),
                daylight_cycle_enabled: sessions.daylight_cycle_enabled(),
                weather: sessions.weather(),
                players_sleeping_percentage: sessions.players_sleeping_percentage(),
                keep_inventory: sessions.keep_inventory(),
                simulation_tick,
                world_flush_plan,
            })
        }))
    }
}
