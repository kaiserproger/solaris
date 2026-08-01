#[cfg(test)]
use mc_entity::SpawnEntity;
use mc_entity::{RegionOwnerLaneError, Vec3, VillagerBindingClaim};

use super::SessionRegistry;

impl SessionRegistry {
    pub(crate) async fn claim_script_villager_binding(
        &self,
        center: Vec3,
        radius: f64,
        token: String,
    ) -> Result<Option<VillagerBindingClaim>, RegionOwnerLaneError> {
        let owner = self.entities.handle.clone();
        let result = tokio::task::spawn_blocking(move || {
            owner.claim_nearest_villager(center, radius, token)
        })
        .await
        .map_err(|_| RegionOwnerLaneError::Closed)?;
        self.entities.try_resolve(result)
    }

    pub(crate) async fn apply_script_villager_binding_goal(
        &self,
        token: String,
        goal: mc_entity::GoalState,
    ) -> Result<bool, RegionOwnerLaneError> {
        let owner = self.entities.handle.clone();
        let result =
            tokio::task::spawn_blocking(move || owner.apply_villager_binding_goal(token, goal))
                .await
                .map_err(|_| RegionOwnerLaneError::Closed)?;
        let applied = self.entities.try_resolve(result)?;
        if let Some(entity) = applied {
            self.track_villager_override(entity);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    #[cfg(test)]
    pub(crate) fn spawn_script_villager_for_test(&self, position: Vec3) -> mc_entity::EntityId {
        let mut entities = self.lock_entities("spawn script villager test fixture");
        entities.spawn(SpawnEntity::new(119, "minecraft:villager", position))
    }

    #[cfg(test)]
    pub(crate) fn script_entity_goal_for_test(
        &self,
        entity: mc_entity::EntityId,
    ) -> Option<mc_entity::GoalState> {
        self.lock_entities("read script villager test fixture")
            .snapshot(entity)
            .map(|snapshot| snapshot.goal)
    }
}
