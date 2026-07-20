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
        tokio::task::spawn_blocking(move || owner.claim_nearest_villager(center, radius, token))
            .await
            .map_err(|_| RegionOwnerLaneError::Closed)?
    }

    #[cfg(test)]
    pub(crate) fn spawn_script_villager_for_test(&self, position: Vec3) {
        let mut entities = self.lock_entities("spawn script villager test fixture");
        entities.spawn(SpawnEntity::new(119, "minecraft:villager", position));
    }
}
