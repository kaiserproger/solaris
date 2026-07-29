use std::collections::HashMap;
use std::sync::atomic::Ordering;

use mc_entity::{EntitySnapshot, GoalState, RegionKey, SpawnEntity};

use crate::play::is_hostile_entity;

use super::entity_goal_defaults::apply_default_mob_goal;
use super::entity_lifecycle::track_entity_chunk_locked;
use super::interaction_geometry::entity_aabb;
use super::outbound::dispatch_visibility_commands;
use super::visibility::{
    initialize_entity_wire_state_from_snapshot_locked,
    install_committed_entity_publications_locked, server_entity_snapshot_from,
};
use super::{SessionRegistry, apply_entity_facts};

const PUBLICATION_BATCH_SIZE: usize = 2_048;

#[derive(Debug, Clone, Copy)]
pub(crate) struct LoadBenchSeedStats {
    pub(crate) entities: usize,
    pub(crate) hostile_entities: usize,
    pub(crate) regions: usize,
    pub(crate) max_entities_per_region: usize,
    pub(crate) spawn_dispatches: usize,
    pub(crate) owner_lanes: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LoadBenchReadinessStats {
    pub(crate) sessions: usize,
    pub(crate) desired_chunks: usize,
    pub(crate) desired_loaded_chunks: usize,
    pub(crate) pending_chunks: usize,
    pub(crate) min_desired_loaded_chunks: usize,
    pub(crate) max_desired_loaded_chunks: usize,
    pub(crate) visible_entity_links: usize,
    pub(crate) owner_entities: usize,
    pub(crate) active_simulation_entities: usize,
    pub(crate) active_hostile_entities: usize,
    pub(crate) prepared_chunks: usize,
    pub(crate) prepared_in_flight: usize,
    pub(crate) pending_subscriber_chunks: usize,
    pub(crate) pending_subscribers: usize,
    pub(crate) entity_update_budget_per_lane: usize,
    pub(crate) entity_update_budget_total: usize,
    pub(crate) entity_update_selected: usize,
    pub(crate) entity_update_active_population: usize,
    pub(crate) entity_update_rotation_ticks: usize,
    pub(crate) entity_movement_publication_budget: usize,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LoadBenchActivityStats {
    pub(crate) active_simulation_entities: usize,
    pub(crate) active_hostile_entities: usize,
    pub(crate) entity_update_budget_per_lane: usize,
    pub(crate) entity_update_budget_total: usize,
    pub(crate) entity_update_selected: usize,
    pub(crate) entity_update_active_population: usize,
    pub(crate) entity_update_rotation_ticks: usize,
    pub(crate) entity_movement_publication_budget: usize,
}

impl SessionRegistry {
    pub(crate) fn seed_load_bench_entities(
        &self,
        mut entities: Vec<SpawnEntity>,
    ) -> LoadBenchSeedStats {
        let mob_behaviors = self.mob_behavior_table();
        let spawn_tick = self.entity_lifecycle_tick.load(Ordering::Acquire);
        let mut region_counts = HashMap::<RegionKey, usize>::new();
        let mut hostile_entities = 0usize;

        for entity in &mut entities {
            apply_entity_facts(entity);
            apply_default_mob_goal(entity, &mob_behaviors);
            entity.goal = GoalState::Wander {
                speed: 0.04,
                period_ticks: 1,
            };
            entity.retained.spawn_tick = spawn_tick;
            hostile_entities += usize::from(is_hostile_entity(&entity.type_name));
            if let Some(region) = RegionKey::from_position(entity.position) {
                *region_counts.entry(region).or_default() += 1;
            }
        }

        let snapshots = self
            .entities
            .handle
            .spawn_unique_batch(entities)
            .unwrap_or_else(|error| panic!("load benchmark owner batch failed: {error:?}"));
        let entities = snapshots.len();
        self.pressure_observation.record_entity_inserts(entities);
        let mut spawn_dispatches = 0usize;
        for batch in snapshots.chunks(PUBLICATION_BATCH_SIZE) {
            spawn_dispatches += self.publish_load_bench_entity_batch(batch);
        }

        LoadBenchSeedStats {
            entities,
            hostile_entities,
            regions: region_counts.len(),
            max_entities_per_region: region_counts.values().copied().max().unwrap_or(0),
            spawn_dispatches,
            owner_lanes: self.entities.status().lane_count,
        }
    }

    pub(crate) fn load_bench_readiness(&self) -> LoadBenchReadinessStats {
        let inner = self.lock_inner("snapshot load benchmark readiness");
        let sessions = inner.sessions.len();
        let mut desired_chunks = 0usize;
        let mut desired_loaded_chunks = 0usize;
        let mut min_desired_loaded_chunks = usize::MAX;
        let mut max_desired_loaded_chunks = 0usize;
        let mut visible_entity_links = 0usize;
        for session in inner.sessions.values() {
            let loaded = session
                .desired
                .iter()
                .filter(|chunk| session.loaded.contains(chunk))
                .count();
            desired_chunks += session.desired.len();
            desired_loaded_chunks += loaded;
            min_desired_loaded_chunks = min_desired_loaded_chunks.min(loaded);
            max_desired_loaded_chunks = max_desired_loaded_chunks.max(loaded);
            visible_entity_links += session.visible_entities.snapshot().len();
        }
        drop(inner);
        let (prepared_chunks, prepared_in_flight, pending_subscriber_chunks, pending_subscribers) = {
            let cache = self.lock_prepared_cache("snapshot load benchmark prepared readiness");
            (
                cache.prepared.len(),
                cache.prepared_in_flight.len(),
                cache.pending_subscriber_counts.len(),
                cache.pending_subscriber_counts.values().copied().sum(),
            )
        };
        let owner_entities = self.entities.status().entity_count;
        LoadBenchReadinessStats {
            sessions,
            desired_chunks,
            desired_loaded_chunks,
            pending_chunks: desired_chunks.saturating_sub(desired_loaded_chunks),
            min_desired_loaded_chunks: if sessions == 0 {
                0
            } else {
                min_desired_loaded_chunks
            },
            max_desired_loaded_chunks,
            visible_entity_links,
            owner_entities,
            active_simulation_entities: self.active_simulation_entities.load().len(),
            active_hostile_entities: self.active_hostile_entities.load().len(),
            prepared_chunks,
            prepared_in_flight,
            pending_subscriber_chunks,
            pending_subscribers,
            entity_update_budget_per_lane: self
                .entity_update_budget_per_lane
                .load(Ordering::Relaxed),
            entity_update_budget_total: self.entity_update_budget_total.load(Ordering::Relaxed),
            entity_update_selected: self.entity_update_selected.load(Ordering::Relaxed),
            entity_update_active_population: self
                .entity_update_active_population
                .load(Ordering::Relaxed),
            entity_update_rotation_ticks: self
                .entity_update_active_population
                .load(Ordering::Relaxed)
                .div_ceil(
                    self.entity_update_budget_total
                        .load(Ordering::Relaxed)
                        .max(1),
                ),
            entity_movement_publication_budget: self.entity_movement_publication_budget(),
        }
    }

    pub(crate) fn load_bench_activity(&self) -> LoadBenchActivityStats {
        let entity_update_budget_total = self.entity_update_budget_total.load(Ordering::Relaxed);
        let entity_update_active_population =
            self.entity_update_active_population.load(Ordering::Relaxed);
        LoadBenchActivityStats {
            active_simulation_entities: self.active_simulation_entities.load().len(),
            active_hostile_entities: self.active_hostile_entities.load().len(),
            entity_update_budget_per_lane: self
                .entity_update_budget_per_lane
                .load(Ordering::Relaxed),
            entity_update_budget_total,
            entity_update_selected: self.entity_update_selected.load(Ordering::Relaxed),
            entity_update_active_population,
            entity_update_rotation_ticks: entity_update_active_population
                .div_ceil(entity_update_budget_total.max(1)),
            entity_movement_publication_budget: self.entity_movement_publication_budget(),
        }
    }

    fn publish_load_bench_entity_batch(&self, snapshots: &[EntitySnapshot]) -> usize {
        let server_snapshots = snapshots
            .iter()
            .cloned()
            .map(server_entity_snapshot_from)
            .collect::<Vec<_>>();
        let mut inner = self.lock_inner("publish load benchmark entity batch");
        for snapshot in &server_snapshots {
            if is_hostile_entity(&snapshot.type_name) {
                inner.hostile_entities.insert(snapshot.id);
            }
            inner
                .entity_type_aabbs
                .entry(snapshot.type_id)
                .or_insert_with(|| entity_aabb(&snapshot.type_name));
            track_entity_chunk_locked(&mut inner, snapshot.id, snapshot.position);
            initialize_entity_wire_state_from_snapshot_locked(&mut inner, snapshot);
        }
        let dispatches = install_committed_entity_publications_locked(&mut inner, server_snapshots);
        let count = dispatches
            .iter()
            .map(|dispatch| match &dispatch.command {
                super::outbound::OutboundCommand::SpawnEntity(_) => 1,
                super::outbound::OutboundCommand::SpawnEntities(entities) => entities.len(),
                _ => 0,
            })
            .sum();
        drop(inner);
        dispatch_visibility_commands(dispatches);
        count
    }
}
