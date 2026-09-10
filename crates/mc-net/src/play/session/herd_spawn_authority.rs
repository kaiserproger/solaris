use mc_entity::SpawnEntity;
use mc_entity::natural_spawn_26_1_2::{entity_type_uses_aquatic_physics, is_hostile_entity};
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
pub(super) fn limit_natural_candidates(
    candidates: Vec<SpawnEntity>,
    (hostile_capacity, ground_capacity, aquatic_capacity): (usize, usize, usize),
) -> Vec<SpawnEntity> {
    let mut accepted_hostiles = 0;
    let mut accepted_ground = 0;
    let mut accepted_aquatic = 0;
    candidates
        .into_iter()
        .filter(|candidate| {
            if is_hostile_entity(&candidate.type_name) {
                if accepted_hostiles >= hostile_capacity {
                    return false;
                }
                accepted_hostiles += 1;
            } else if entity_type_uses_aquatic_physics(&candidate.type_name) {
                if accepted_aquatic >= aquatic_capacity {
                    return false;
                }
                accepted_aquatic += 1;
            } else {
                if accepted_ground >= ground_capacity {
                    return false;
                }
                accepted_ground += 1;
            }
            true
        })
        .collect()
}
