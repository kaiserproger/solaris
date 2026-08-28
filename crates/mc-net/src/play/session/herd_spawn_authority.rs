use std::ops::Deref;

#[cfg(test)]
use super::SessionRegistry;
use super::outbound::VisibilityDispatch;

mod commit;
#[cfg(test)]
mod legacy;
mod periodic;

#[cfg(any(test, feature = "load-bench"))]
pub(in crate::play::session) use commit::install_committed_herd_spawns_locked;
#[cfg(test)]
pub(in crate::play::session) use legacy::{
    ChunkHerdClaimProbe, ClaimedPendingHostiles, claim_loaded_pending_hostiles_locked,
};
#[cfg(test)]
pub(crate) use mc_entity::natural_spawn_26_1_2::NaturalSpawnReport;
pub(crate) use mc_entity::natural_spawn_26_1_2::NaturalSpawnScheduler;
#[cfg(test)]
pub(super) use mc_entity::natural_spawn_26_1_2::spawn_far_enough_from_players;
#[cfg(test)]
pub(super) use mc_entity::natural_spawn_26_1_2::{
    VANILLA_CREATURE_MOB_CAP, VANILLA_HOSTILE_MOB_CAP, VANILLA_WATER_CREATURE_MOB_CAP,
};
pub(crate) use periodic::NaturalSpawnTickInput;

#[derive(Debug)]
pub(in crate::play) struct HerdSpawnOutcome {
    pub(in crate::play::session) dispatches: Vec<VisibilityDispatch>,
    #[cfg(test)]
    retryable_chunks: Vec<(i32, i32)>,
}

impl HerdSpawnOutcome {
    pub(in crate::play::session) fn committed(dispatches: Vec<VisibilityDispatch>) -> Self {
        Self {
            dispatches,
            #[cfg(test)]
            retryable_chunks: Vec::new(),
        }
    }

    #[cfg(test)]
    fn retryable(chunks: Vec<(i32, i32)>) -> Self {
        Self {
            dispatches: Vec::new(),
            retryable_chunks: chunks,
        }
    }

    #[cfg(test)]
    pub(in crate::play) fn retryable_chunks(&self) -> &[(i32, i32)] {
        &self.retryable_chunks
    }

    pub(in crate::play) fn into_dispatches(self) -> Vec<VisibilityDispatch> {
        self.dispatches
    }
}

impl Deref for HerdSpawnOutcome {
    type Target = [VisibilityDispatch];

    fn deref(&self) -> &Self::Target {
        &self.dispatches
    }
}

#[cfg(test)]
impl SessionRegistry {
    pub(in crate::play) fn activate_pending_hostiles_owned(
        &self,
        _authority: &crate::play::simulation::SimulationAuthority,
    ) -> HerdSpawnOutcome {
        self.activate_pending_hostiles_legacy()
    }
}
