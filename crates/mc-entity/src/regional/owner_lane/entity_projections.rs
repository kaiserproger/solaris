use std::collections::{BTreeMap, HashSet};
use std::sync::mpsc::{Receiver, channel};

use super::{RegionOwnerLaneError, RegionOwnerLaneMessage, RegionalOwnerLaneReader};
use crate::entity_projections::EntityProjection;
use crate::regional::{RegionKey, RegionLease};
use crate::{EntityDespawnProjection, EntityId, EntitySimulationProjection, EntityStore};

impl RegionalOwnerLaneReader {
    pub(in crate::regional) fn request_simulation_projections_for_ids(
        &self,
        entities: Vec<(RegionLease, EntityId)>,
    ) -> Result<
        Receiver<Result<Vec<EntitySimulationProjection>, RegionOwnerLaneError>>,
        RegionOwnerLaneError,
    > {
        let (reply, projections) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::SimulationProjectionsForIds { entities, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(projections)
    }

    pub(in crate::regional) fn request_despawn_projections_for_ids(
        &self,
        entities: Vec<(RegionLease, EntityId)>,
    ) -> Result<
        Receiver<Result<Vec<EntityDespawnProjection>, RegionOwnerLaneError>>,
        RegionOwnerLaneError,
    > {
        let (reply, projections) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::DespawnProjectionsForIds { entities, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(projections)
    }
}

pub(super) fn read<P: EntityProjection>(
    lane: usize,
    regions: &BTreeMap<RegionKey, (RegionLease, EntityStore)>,
    entities: Vec<(RegionLease, EntityId)>,
) -> Result<Vec<P>, RegionOwnerLaneError> {
    let mut projections = Vec::with_capacity(entities.len());
    let mut ids_by_region = BTreeMap::<RegionKey, HashSet<EntityId>>::new();
    for (lease, entity) in entities {
        if lease.lane != lane {
            return Err(RegionOwnerLaneError::WrongLane);
        }
        let Some((current, _)) = regions.get(&lease.key) else {
            return Err(RegionOwnerLaneError::UnknownRegion);
        };
        if *current != lease {
            return Err(RegionOwnerLaneError::StaleLease);
        }
        ids_by_region.entry(lease.key).or_default().insert(entity);
    }
    for (key, ids) in ids_by_region {
        let Some((_, store)) = regions.get(&key) else {
            return Err(RegionOwnerLaneError::UnknownRegion);
        };
        projections.extend(store.projections_for_ids::<P>(&ids));
    }
    projections.sort_unstable_by_key(EntityProjection::id);
    Ok(projections)
}
