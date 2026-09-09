use std::collections::{BTreeMap, HashSet};
use std::sync::mpsc::{Receiver, channel};

use super::{
    RegionKey, RegionLease, RegionOwnerLaneError, RegionalOwnerCommand, RegionalOwnerCoordinator,
    RegionalOwnerHandle, RegionalOwnerLaneReader,
};
use crate::entity_projections::EntityProjection;
use crate::{EntityDespawnProjection, EntityId, EntitySimulationProjection};

impl RegionalOwnerHandle {
    pub fn simulation_projections_for_ids(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<Vec<EntitySimulationProjection>, RegionOwnerLaneError> {
        if entities.is_empty() {
            return Ok(Vec::new());
        }
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SimulationProjectionsForIds {
                entities: entities.clone(),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn despawn_projections_for_ids(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<Vec<EntityDespawnProjection>, RegionOwnerLaneError> {
        if entities.is_empty() {
            return Ok(Vec::new());
        }
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::DespawnProjectionsForIds {
                entities: entities.clone(),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }
}

impl RegionalOwnerCoordinator {
    pub(super) fn simulation_projections_for_ids(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<Vec<EntitySimulationProjection>, RegionOwnerLaneError> {
        self.entity_projections_for_ids(
            entities,
            RegionalOwnerLaneReader::request_simulation_projections_for_ids,
        )
    }

    pub(super) fn despawn_projections_for_ids(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<Vec<EntityDespawnProjection>, RegionOwnerLaneError> {
        self.entity_projections_for_ids(
            entities,
            RegionalOwnerLaneReader::request_despawn_projections_for_ids,
        )
    }

    fn entity_projections_for_ids<P: EntityProjection>(
        &self,
        entities: &HashSet<EntityId>,
        request: impl Fn(
            &RegionalOwnerLaneReader,
            Vec<(RegionLease, EntityId)>,
        )
            -> Result<Receiver<Result<Vec<P>, RegionOwnerLaneError>>, RegionOwnerLaneError>,
    ) -> Result<Vec<P>, RegionOwnerLaneError> {
        self.commit_state.ensure_committed_state()?;
        let mut ordered = entities.iter().copied().collect::<Vec<_>>();
        ordered.sort_unstable();
        let mut requests = BTreeMap::<usize, Vec<(RegionLease, EntityId)>>::new();
        for entity in ordered {
            let Some(expected_key) = self.locations.get(&entity).copied() else {
                continue;
            };
            let lease = self
                .ownership
                .lease(expected_key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            requests
                .entry(lease.lane)
                .or_default()
                .push((lease, entity));
        }
        let mut pending = Vec::with_capacity(requests.len());
        for (lane, entities) in requests {
            let owner = self
                .lanes
                .get(&lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?;
            pending.push(request(&owner.reader(), entities)?);
        }
        let mut projections = Vec::with_capacity(entities.len());
        for completion in pending {
            projections.extend(
                completion
                    .recv()
                    .map_err(|_| RegionOwnerLaneError::Closed)??,
            );
        }
        projections.sort_unstable_by_key(EntityProjection::id);
        if projections.iter().any(|projection| {
            !entities.contains(&projection.id())
                || self.locations.get(&projection.id()).copied()
                    != RegionKey::from_position(projection.position())
        }) {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        Ok(projections)
    }
}
