use mc_physics::BlockMaterialIds;
use mc_world::WorldReadView;

use crate::play::{
    NaturalSpawnScheduler, NaturalSpawnTickInput, RandomTickPolicy, SessionRegistry,
};

pub(super) struct NaturalSpawnTicker {
    scheduler: NaturalSpawnScheduler,
    friendly_interval_ticks: u64,
    hostile_interval_ticks: u64,
    simulation_distance: i32,
}

impl NaturalSpawnTicker {
    pub(super) fn new(policy: RandomTickPolicy) -> Self {
        Self {
            scheduler: NaturalSpawnScheduler::default(),
            friendly_interval_ticks: policy.friendly_spawn_interval_ticks,
            hostile_interval_ticks: policy.hostile_spawn_interval_ticks,
            simulation_distance: policy.simulation_distance,
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
                friendly_interval: self.friendly_interval_ticks,
                hostile_interval: self.hostile_interval_ticks,
                simulation_distance: self.simulation_distance,
                world_read,
                materials,
            },
        );
    }
}
