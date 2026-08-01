use mc_entity::{EntitySnapshot, SpawnEntity};

use crate::play::is_hostile_entity;

use super::super::entity_lifecycle::{
    track_entity_chunk_locked, update_breeding_tick_tracking_locked,
};
use super::super::entity_owner::owner_result;
use super::super::entity_physics_class::entity_type_uses_aquatic_physics;
use super::super::interaction_geometry::entity_aabb;
use super::super::visibility::{
    initialize_entity_wire_state_from_snapshot_locked, server_entity_snapshot_from,
    spawn_entity_visibility_from_snapshot_locked,
};
use super::super::{SessionRegistry, SessionRegistryInner};
use super::VisibilityDispatch;

impl SessionRegistry {
    pub(super) fn commit_unique_herd_candidates(
        &self,
        candidates: Vec<SpawnEntity>,
    ) -> Result<Vec<EntitySnapshot>, ()> {
        let candidate_uuids = candidates
            .iter()
            .filter_map(|candidate| candidate.uuid)
            .collect::<Vec<_>>();
        let mut entities = self.lock_entities("commit unique herd batch");
        match entities.try_spawn_unique_batch(candidates) {
            Ok(committed) => Ok(committed),
            Err(mc_entity::RegionOwnerLaneError::Journal) => {
                let failure = self.entities.take_journal_failure(candidate_uuids);
                if failure.is_some_and(|error| !error.outcome_unknown()) {
                    Err(())
                } else {
                    owner_result(
                        &self.entities,
                        Err(mc_entity::RegionOwnerLaneError::Journal),
                    )
                }
            }
            Err(error) => owner_result(&self.entities, Err(error)),
        }
    }
}

pub(in crate::play::session) fn install_committed_herd_spawns_locked(
    inner: &mut SessionRegistryInner,
    committed: Vec<EntitySnapshot>,
    _lifecycle_tick: u64,
) -> Vec<VisibilityDispatch> {
    let mut snapshots = Vec::with_capacity(committed.len());
    for entity in committed {
        update_breeding_tick_tracking_locked(inner, entity.id, entity.animal);
        if entity.type_name == "minecraft:sheep" {
            inner.sheep_entities.insert(entity.id);
        }
        if is_hostile_entity(&entity.type_name) {
            inner.hostile_entities.insert(entity.id);
            inner.natural_hostile_mobs.insert(entity.id);
        } else if entity_type_uses_aquatic_physics(&entity.type_name) {
            inner.natural_aquatic_mobs.insert(entity.id);
        } else {
            inner.natural_ground_mobs.insert(entity.id);
        }
        let aabb = entity_aabb(&entity.type_name);
        let snapshot = server_entity_snapshot_from(entity);
        inner
            .entity_type_aabbs
            .entry(snapshot.type_id)
            .or_insert(aabb);
        track_entity_chunk_locked(inner, snapshot.id, snapshot.position);
        initialize_entity_wire_state_from_snapshot_locked(inner, &snapshot);
        snapshots.push(snapshot);
    }
    snapshots
        .into_iter()
        .flat_map(|snapshot| spawn_entity_visibility_from_snapshot_locked(inner, snapshot))
        .collect()
}
