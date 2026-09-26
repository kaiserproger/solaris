//! Session/entity-owner paths used by durable resident identity.
//!
//! The regional entity owner is the single authority for whether a bound NPC is
//! loaded, dead, or converted; this endpoint adds no second roster.

use mc_entity::{
    AttributeKind, EntitySnapshot, PreparedSnapshotMutation, RegionOwnerLaneError, SpawnEntity,
    Vec3,
};
use uuid::Uuid;

use super::SessionRegistry;
use super::entity_lifecycle::track_entity_chunk_locked;
use super::visibility::{
    initialize_entity_wire_state_from_snapshot_locked, server_entity_snapshot_from,
    spawn_entity_visibility_from_snapshot_locked,
};

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

    /// Compute the exact native after-image before encoding a world decision.
    /// It is fenced again by the regional owner inside the simulation turn.
    pub(crate) fn plan_resident_treatment(
        expected: &EntitySnapshot,
        operation_revision: u64,
        heal_milli: u32,
    ) -> Result<EntitySnapshot, RegionOwnerLaneError> {
        let max = expected
            .attributes
            .base(&AttributeKind::MaxHealth)
            .ok_or(RegionOwnerLaneError::InvalidMutation)? as f32;
        if !max.is_finite()
            || !expected.health.is_finite()
            || expected.health <= 0.0
            || expected.health >= max
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        let mut next = expected.clone();
        next.health = (expected.health + heal_milli as f32 / 1000.0).min(max);
        next.retained.treatment_decision_id = operation_revision;
        Ok(next)
    }

    /// Hold the native patient's regional fence only within the owning
    /// simulation command turn, before the warehouse WAL can debit supplies.
    pub(crate) async fn prepare_resident_treatment(
        &self,
        expected: EntitySnapshot,
        operation_revision: u64,
        heal_milli: u32,
    ) -> Result<Option<(PreparedSnapshotMutation, EntitySnapshot)>, RegionOwnerLaneError> {
        let next = Self::plan_resident_treatment(&expected, operation_revision, heal_milli)?;
        let owner = self.entities.handle.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            owner
                .prepare_snapshot_if_current(expected, next.clone())
                .map(|prepared| prepared.map(|prepared| (prepared, next)))
        })
        .await
        .map_err(|_| RegionOwnerLaneError::Closed)?;
        self.entities.try_resolve(prepared)
    }

    /// Publish native health only after its regional journal commit is durable.
    pub(crate) fn publish_resident_treatment(&self, accepted: &EntitySnapshot) {
        let mut inner = self.lock_session_entities("publish resident treatment");
        let dispatches =
            super::entity_combat::publish_accepted_entity_health_locked(&mut inner, accepted);
        drop(inner);
        super::dispatch_visibility_commands(dispatches);
    }

    /// Replay a paid world decision after entity restoration and before clients
    /// or the storage actor can observe either participant.
    pub(crate) fn replay_resident_treatment(
        &self,
        expected: EntitySnapshot,
        next: EntitySnapshot,
    ) -> Result<bool, RegionOwnerLaneError> {
        self.lock_session_entities("replay paid resident treatment")
            .entities
            .try_replace_snapshot_if_current(expected, next)
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
        let snapshot = self
            .entities
            .try_resolve(snapshot)?
            .ok_or(RegionOwnerLaneError::OutcomeUnknown)?;
        let published = server_entity_snapshot_from(snapshot.clone());
        let mut inner = self.lock_session_entities("publish resident spawn");
        inner
            .entity_type_aabbs
            .entry(published.type_id)
            .or_insert_with(|| super::interaction_geometry::entity_aabb(&snapshot.type_name));
        track_entity_chunk_locked(&mut inner, published.id, published.position);
        initialize_entity_wire_state_from_snapshot_locked(&mut inner, &published);
        let dispatches = spawn_entity_visibility_from_snapshot_locked(&mut inner, published);
        drop(inner);
        super::dispatch_visibility_commands(dispatches);
        Ok(snapshot)
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

    #[cfg(test)]
    pub(crate) fn remove_resident_entity_for_test(&self, entity: mc_entity::EntityId) -> bool {
        let mut entities = self.lock_entities("remove resident test fixture");
        let Some(expected) = entities.snapshot(entity) else {
            return false;
        };
        entities.remove_if_current(expected).is_some()
    }
}
