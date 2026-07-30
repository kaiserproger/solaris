use std::sync::mpsc::{Receiver, SyncSender};

use mc_entity::{SpawnEntity, Vec3};
use tracing::debug;

use crate::play::simulation::SimulationAuthority;
use crate::play::{
    HerdSpawn, MIN_ENTITY_SPAWN_DISTANCE_FROM_PLAYER, is_hostile_entity, world_time_is_night,
};

use super::super::entity_physics_class::entity_type_uses_aquatic_physics;
use super::super::{SessionRegistry, SessionRegistryInner};
use super::commit::install_committed_herd_spawns_locked;
use super::planning::build_herd_spawn_candidates;
use super::{
    HerdSpawnOutcome, VANILLA_CREATURE_MOB_CAP, VANILLA_HOSTILE_MOB_CAP,
    VANILLA_WATER_CREATURE_MOB_CAP,
};

#[derive(Debug)]
struct ChunkHerdClaim {
    chunk: (i32, i32),
    was_spawned: bool,
    pending_before: Option<Vec<HerdSpawn>>,
}

#[derive(Debug)]
struct PendingHostileClaim {
    chunk: (i32, i32),
    spawns: Vec<HerdSpawn>,
}

#[derive(Debug, Default)]
pub(in crate::play::session) struct ClaimedPendingHostiles {
    chunks: Vec<PendingHostileClaim>,
    player_positions: Vec<Vec3>,
    hostile_capacity: usize,
    ground_capacity: usize,
    aquatic_capacity: usize,
}

#[derive(Debug)]
pub(in crate::play::session) struct ChunkHerdClaimProbe {
    reached: SyncSender<()>,
    resume: Receiver<()>,
}

impl SessionRegistry {
    pub(in crate::play) fn install_chunk_herd_claim_probe_for_test(
        &self,
        reached: SyncSender<()>,
        resume: Receiver<()>,
    ) {
        *self
            .chunk_herd_claim_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(ChunkHerdClaimProbe { reached, resume });
    }

    #[cfg(test)]
    fn pause_before_chunk_herd_claim_for_test(&self) {
        let probe = self
            .chunk_herd_claim_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(probe) = probe {
            probe
                .reached
                .send(())
                .expect("chunk herd claim probe receiver");
            probe.resume.recv().expect("chunk herd claim probe release");
        }
    }

    pub(in crate::play) fn ensure_chunk_herd(
        &self,
        _authority: &SimulationAuthority,
        chunk: (i32, i32),
        spawns: &[HerdSpawn],
    ) -> HerdSpawnOutcome {
        self.ensure_chunk_herds_owned(
            &[(chunk, spawns.to_vec())],
            MIN_ENTITY_SPAWN_DISTANCE_FROM_PLAYER,
        )
    }

    pub(in crate::play) fn ensure_chunk_herds(
        &self,
        _authority: &SimulationAuthority,
        herds: &[((i32, i32), Vec<HerdSpawn>)],
    ) -> HerdSpawnOutcome {
        self.ensure_chunk_herds_owned(herds, MIN_ENTITY_SPAWN_DISTANCE_FROM_PLAYER)
    }

    pub(in crate::play) fn ensure_chunk_herd_legacy_for_test(
        &self,
        chunk: (i32, i32),
        spawns: &[HerdSpawn],
    ) -> Vec<super::VisibilityDispatch> {
        self.ensure_chunk_herds_owned(&[(chunk, spawns.to_vec())], 0.5)
            .into_dispatches()
    }

    fn ensure_chunk_herds_owned(
        &self,
        herds: &[((i32, i32), Vec<HerdSpawn>)],
        minimum_player_distance: f64,
    ) -> HerdSpawnOutcome {
        self.pause_before_chunk_herd_claim_for_test();
        let (selected, claims, player_positions, capacities) = {
            let mut inner = self.lock_inner("claim chunk herd");
            let nighttime = world_time_is_night(self.world_time());
            let mut selected = Vec::with_capacity(herds.len());
            let mut claims = Vec::with_capacity(herds.len());
            for (chunk, spawns) in herds {
                let claim = ChunkHerdClaim {
                    chunk: *chunk,
                    was_spawned: inner.spawned_entity_chunks.contains(chunk),
                    pending_before: inner.pending_hostile_spawns.get(chunk).cloned(),
                };
                let spawns = if !inner.spawned_entity_chunks.insert(*chunk) {
                    if nighttime && inner.loaded_chunk_refcounts.contains_key(chunk) {
                        inner.pending_hostile_spawns.remove(chunk)
                    } else {
                        None
                    }
                } else if nighttime {
                    Some(spawns.clone())
                } else {
                    let (pending_hostiles, immediate): (Vec<_>, Vec<_>) =
                        spawns.iter().cloned().partition(|spawn| spawn.hostile);
                    if !pending_hostiles.is_empty() {
                        inner
                            .pending_hostile_spawns
                            .insert(*chunk, pending_hostiles);
                    }
                    Some(immediate)
                };
                if let Some(spawns) = spawns {
                    claims.push(claim);
                    selected.push((*chunk, spawns));
                }
            }
            (
                selected,
                claims,
                session_player_positions(&inner),
                (
                    VANILLA_HOSTILE_MOB_CAP.saturating_sub(inner.hostile_entities.len()),
                    VANILLA_CREATURE_MOB_CAP.saturating_sub(inner.natural_ground_mobs.len()),
                    VANILLA_WATER_CREATURE_MOB_CAP.saturating_sub(inner.natural_aquatic_mobs.len()),
                ),
            )
        };
        if selected.is_empty() {
            return HerdSpawnOutcome::committed(Vec::new());
        }
        let lifecycle_tick = self.simulation_tick();
        let mob_behaviors = self.mob_behavior_table();
        let candidates = selected
            .iter()
            .flat_map(|(chunk, spawns)| {
                build_herd_spawn_candidates(
                    *chunk,
                    spawns,
                    &player_positions,
                    lifecycle_tick,
                    minimum_player_distance,
                    &mob_behaviors,
                )
            })
            .collect::<Vec<_>>();
        let candidates = limit_natural_candidates(candidates, capacities);
        let committed = match self.commit_unique_herd_candidates(candidates) {
            Ok(committed) => committed,
            Err(()) => {
                let retryable_chunks = claims.iter().map(|claim| claim.chunk).collect();
                let mut inner = self.lock_inner("restore safe chunk herd claims");
                restore_chunk_herd_claims_locked(&mut inner, claims);
                return HerdSpawnOutcome::retryable(retryable_chunks);
            }
        };
        let committed_count = committed.len();
        let mut inner = self.lock_inner("publish committed chunk herd");
        let dispatches =
            install_committed_herd_spawns_locked(&mut inner, committed, lifecycle_tick);
        debug!(
            chunks = selected.len(),
            entities = committed_count,
            "materialized chunk entity spawn batch"
        );
        HerdSpawnOutcome::committed(dispatches)
    }

    pub(super) fn activate_pending_hostiles_legacy(&self) -> HerdSpawnOutcome {
        let claimed = {
            let mut inner = self.lock_inner("claim loaded pending hostile chunks");
            claim_loaded_pending_hostiles_locked(&mut inner)
        };
        self.commit_claimed_pending_hostiles(claimed)
    }

    pub(in crate::play::session) fn commit_claimed_pending_hostiles(
        &self,
        claimed: ClaimedPendingHostiles,
    ) -> HerdSpawnOutcome {
        if claimed.chunks.is_empty() {
            return HerdSpawnOutcome::committed(Vec::new());
        }
        let lifecycle_tick = self.simulation_tick();
        let mob_behaviors = self.mob_behavior_table();
        let candidates = claimed
            .chunks
            .iter()
            .flat_map(|claim| {
                build_herd_spawn_candidates(
                    claim.chunk,
                    &claim.spawns,
                    &claimed.player_positions,
                    lifecycle_tick,
                    MIN_ENTITY_SPAWN_DISTANCE_FROM_PLAYER,
                    &mob_behaviors,
                )
            })
            .collect::<Vec<_>>();
        let candidates = limit_natural_candidates(
            candidates,
            (
                claimed.hostile_capacity,
                claimed.ground_capacity,
                claimed.aquatic_capacity,
            ),
        );
        let committed = match self.commit_unique_herd_candidates(candidates) {
            Ok(committed) => committed,
            Err(()) => {
                let retryable_chunks = claimed.chunks.iter().map(|claim| claim.chunk).collect();
                let mut inner = self.lock_inner("restore safe pending hostile claims");
                for claim in claimed.chunks {
                    inner
                        .pending_hostile_spawns
                        .entry(claim.chunk)
                        .or_insert(claim.spawns);
                }
                return HerdSpawnOutcome::retryable(retryable_chunks);
            }
        };
        let committed_count = committed.len();
        let lifecycle_tick = self.simulation_tick();
        let mut inner = self.lock_inner("publish pending hostile batch");
        let dispatches =
            install_committed_herd_spawns_locked(&mut inner, committed, lifecycle_tick);
        debug!(
            chunks = claimed.chunks.len(),
            entities = committed_count,
            "materialized pending hostile batch"
        );
        HerdSpawnOutcome::committed(dispatches)
    }
}

fn session_player_positions(inner: &SessionRegistryInner) -> Vec<Vec3> {
    inner
        .sessions
        .values()
        .map(|session| Vec3::new(session.pose.x, session.pose.y, session.pose.z))
        .collect()
}

pub(in crate::play::session) fn claim_loaded_pending_hostiles_locked(
    inner: &mut SessionRegistryInner,
) -> ClaimedPendingHostiles {
    let loaded_chunks = inner
        .pending_hostile_spawns
        .keys()
        .filter(|chunk| inner.loaded_chunk_refcounts.contains_key(chunk))
        .copied()
        .collect::<Vec<_>>();
    let chunks = loaded_chunks
        .into_iter()
        .filter_map(|chunk| {
            inner
                .pending_hostile_spawns
                .remove(&chunk)
                .map(|spawns| PendingHostileClaim { chunk, spawns })
        })
        .collect();
    ClaimedPendingHostiles {
        chunks,
        player_positions: session_player_positions(inner),
        hostile_capacity: VANILLA_HOSTILE_MOB_CAP.saturating_sub(inner.hostile_entities.len()),
        ground_capacity: VANILLA_CREATURE_MOB_CAP.saturating_sub(inner.natural_ground_mobs.len()),
        aquatic_capacity: VANILLA_WATER_CREATURE_MOB_CAP
            .saturating_sub(inner.natural_aquatic_mobs.len()),
    }
}

fn limit_natural_candidates(
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

fn restore_chunk_herd_claims_locked(inner: &mut SessionRegistryInner, claims: Vec<ChunkHerdClaim>) {
    for claim in claims {
        if claim.was_spawned {
            inner.spawned_entity_chunks.insert(claim.chunk);
        } else {
            inner.spawned_entity_chunks.remove(&claim.chunk);
        }
        if let Some(pending) = claim.pending_before {
            inner.pending_hostile_spawns.insert(claim.chunk, pending);
        } else {
            inner.pending_hostile_spawns.remove(&claim.chunk);
        }
    }
}
