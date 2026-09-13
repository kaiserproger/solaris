//! Session/entity-owner paths used by durable resident identity.
//!
//! The regional entity owner is the single authority for whether a bound NPC is
//! loaded, dead, or converted; this endpoint adds no second roster.

use mc_entity::{EntitySnapshot, RegionOwnerLaneError, SpawnEntity, Vec3};
use uuid::Uuid;

use super::SessionRegistry;

/// The regional entity owner simulates the overworld only, so a resident actor
/// must be authenticated in that dimension to claim an NPC.
pub(super) const RESIDENT_DIMENSION: &str = "minecraft:overworld";

impl SessionRegistry {
    /// Bounded owner lookup for resident snapshots. The result keeps the caller's
    /// ordering and length; an UUID that is not resident resolves to `None`.
    pub(crate) async fn resident_entity_snapshots(
        &self,
        uuids: &[Uuid],
    ) -> Vec<Option<EntitySnapshot>> {
        if uuids.is_empty() {
            return Vec::new();
        }
        let owner = self.entities.handle.clone();
        let expected = uuids.len();
        let uuids = uuids.to_vec();
        let result = tokio::task::spawn_blocking(move || owner.entities_by_uuid(&uuids)).await;
        match result {
            Ok(Ok(snapshots)) if snapshots.len() == expected => snapshots,
            Ok(_) | Err(_) => Vec::new(),
        }
    }

    /// Spawn one resident entity inside the regional owner and return its
    /// committed snapshot.
    pub(crate) async fn spawn_resident_entity(
        &self,
        entity: SpawnEntity,
    ) -> Result<EntitySnapshot, RegionOwnerLaneError> {
        let owner = self.entities.handle.clone();
        let id = tokio::task::spawn_blocking(move || owner.spawn(entity))
            .await
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        let id = self.entities.try_resolve(id)?;
        let owner = self.entities.handle.clone();
        let snapshot = tokio::task::spawn_blocking(move || owner.snapshot(id))
            .await
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        self.entities
            .try_resolve(snapshot)?
            .ok_or(RegionOwnerLaneError::OutcomeUnknown)
    }

    /// Pose of one authenticated actor in the simulated dimension, or `None`
    /// when that session is not live there. Caller-supplied coordinates never
    /// substitute for this check.
    pub(crate) fn resident_actor_position(&self, actor_id: u64) -> Option<Vec3> {
        let inner = self.lock_inner("resident actor position");
        let session = inner.sessions.get(&actor_id)?;
        if session.tx.is_closed() || session.dimension != RESIDENT_DIMENSION {
            return None;
        }
        Some(Vec3::new(session.pose.x, session.pose.y, session.pose.z))
    }

    #[cfg(test)]
    pub(crate) fn resident_entity_count_for_test(&self) -> usize {
        self.entities
            .handle
            .snapshots()
            .map(|snapshots| snapshots.len())
            .unwrap_or_default()
    }

    #[cfg(test)]
    pub(crate) fn spawn_script_villager_for_test(&self, position: Vec3) -> mc_entity::EntityId {
        let mut entities = self.lock_entities("spawn script villager test fixture");
        entities.spawn(SpawnEntity::new(139, "minecraft:villager", position))
    }

    #[cfg(test)]
    pub(crate) fn spawn_resident_entity_for_test(
        &self,
        uuid: Uuid,
        position: Vec3,
    ) -> EntitySnapshot {
        let mut entities = self.lock_entities("spawn resident test fixture");
        let mut entity = SpawnEntity::new(139, "minecraft:villager", position);
        entity.uuid = Some(uuid);
        let id = entities.spawn(entity);
        entities
            .snapshot(id)
            .expect("spawned resident fixture remains resident")
    }

    /// Convert the bound villager into another entity type through the engine's
    /// own conversion path. The bound identity never rebinds: the ledger reads
    /// the conversion as the resident's death.
    #[cfg(test)]
    pub(crate) fn convert_resident_entity_for_test(&self, entity: mc_entity::EntityId) -> bool {
        let mut entities = self.lock_entities("convert resident test fixture");
        let Some(expected) = entities.snapshot(entity) else {
            return false;
        };
        let mut next = expected.clone();
        next.type_id = 140;
        next.type_name = "minecraft:zombie_villager".to_owned();
        entities.convert_snapshot_if_current(expected, next)
    }
}
