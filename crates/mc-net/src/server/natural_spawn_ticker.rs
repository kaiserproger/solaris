use mc_physics::BlockMaterialIds;
use mc_world::WorldReadView;

use crate::play::{
    NaturalSpawnScheduler, NaturalSpawnTickInput, RandomTickPolicy, SessionRegistry,
};

pub(super) struct NaturalSpawnTicker {
    scheduler: NaturalSpawnScheduler,
    policy: RandomTickPolicy,
}

impl NaturalSpawnTicker {
    pub(super) fn new(policy: RandomTickPolicy) -> Self {
        Self {
            scheduler: NaturalSpawnScheduler::default(),
            policy,
        }
    }

    pub(super) fn tick(
        &mut self,
        sessions: &SessionRegistry,
        tick: u64,
        world_read: Option<&WorldReadView>,
        materials: Option<&BlockMaterialIds>,
    ) {
        let despawn_outcome = sessions.tick_natural_mob_despawn(tick);
        sessions.publish_natural_mob_despawn(despawn_outcome);
        sessions.tick_and_dispatch_periodic_natural_spawning(
            &mut self.scheduler,
            NaturalSpawnTickInput {
                tick,
                policy: self.policy,
                world_read,
                materials,
            },
        );
    }
}
