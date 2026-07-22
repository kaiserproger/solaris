use std::ops::Deref;
#[cfg(test)]
use std::sync::mpsc::{Receiver, SyncSender};

use mc_entity::{AttributeKind, EntitySnapshot, GoalState, SpawnEntity, Vec3};
use tracing::debug;

use crate::play::simulation::SimulationAuthority;
use crate::play::{
    HOSTILE_WANDER_SPEED, HerdSpawn, MAX_HOSTILE_SPAWNS_PER_CHUNK, MAX_PASSIVE_SPAWNS_PER_CHUNK,
    MIN_ENTITY_SPAWN_DISTANCE_FROM_PLAYER, PASSIVE_WANDER_SPEED, herd_uuid, is_hostile_entity,
    world_time_is_night,
};

use super::entity_lifecycle::track_entity_chunk_locked;
use super::entity_owner::owner_result;
use super::entity_physics_class::entity_type_uses_aquatic_physics;
use super::interaction_geometry::{distance_sq, entity_aabb};
use super::outbound::VisibilityDispatch;
use super::visibility::{
    initialize_entity_wire_state_from_snapshot_locked, server_entity_snapshot_from,
};
use super::{
    SessionRegistry, SessionRegistryInner, apply_entity_facts,
    install_committed_entity_publications_locked,
};

pub(super) const VANILLA_HOSTILE_MOB_CAP: usize = 70;
pub(super) const VANILLA_CREATURE_MOB_CAP: usize = 10;
pub(super) const VANILLA_WATER_CREATURE_MOB_CAP: usize = 20;

#[derive(Debug)]
pub(in crate::play) struct HerdSpawnOutcome {
    pub(in crate::play::session) dispatches: Vec<VisibilityDispatch>,
    retryable_chunks: Vec<(i32, i32)>,
}

impl HerdSpawnOutcome {
    fn committed(dispatches: Vec<VisibilityDispatch>) -> Self {
        Self {
            dispatches,
            retryable_chunks: Vec::new(),
        }
    }

    fn retryable(chunks: Vec<(i32, i32)>) -> Self {
        Self {
            dispatches: Vec::new(),
            retryable_chunks: chunks,
        }
    }

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

#[cfg(test)]
#[derive(Debug)]
pub(in crate::play::session) struct ChunkHerdClaimProbe {
    reached: SyncSender<()>,
    resume: Receiver<()>,
}

impl SessionRegistry {
    #[cfg(test)]
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

    #[cfg(test)]
    pub(in crate::play) fn ensure_chunk_herd_legacy_for_test(
        &self,
        chunk: (i32, i32),
        spawns: &[HerdSpawn],
    ) -> Vec<VisibilityDispatch> {
        self.ensure_chunk_herds_owned(&[(chunk, spawns.to_vec())], 0.5)
            .into_dispatches()
    }

    fn ensure_chunk_herds_owned(
        &self,
        herds: &[((i32, i32), Vec<HerdSpawn>)],
        minimum_player_distance: f64,
    ) -> HerdSpawnOutcome {
        #[cfg(test)]
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
        let candidates = selected
            .iter()
            .flat_map(|(chunk, spawns)| {
                build_herd_spawn_candidates(
                    *chunk,
                    spawns,
                    &player_positions,
                    lifecycle_tick,
                    minimum_player_distance,
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

    #[cfg(test)]
    pub(in crate::play) fn activate_pending_hostiles_owned(
        &self,
        _authority: &SimulationAuthority,
    ) -> HerdSpawnOutcome {
        self.activate_pending_hostiles()
    }

    #[cfg(test)]
    fn activate_pending_hostiles(&self) -> HerdSpawnOutcome {
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

    fn commit_unique_herd_candidates(
        &self,
        candidates: Vec<SpawnEntity>,
    ) -> Result<Vec<EntitySnapshot>, ()> {
        let candidate_uuids = candidates
            .iter()
            .filter_map(|candidate| candidate.uuid)
            .collect::<Vec<_>>();
        let mut entities = self.lock_entities("commit unique herd batch");
        match entities.try_spawn_unique_batch(candidates) {
            Ok(committed) => Ok(committed),
            Err(mc_entity::RegionOwnerLaneError::Journal) => {
                let failure = self.entities.take_journal_failure(candidate_uuids);
                if failure.is_some_and(|error| !error.outcome_unknown()) {
                    Err(())
                } else {
                    owner_result(Err(mc_entity::RegionOwnerLaneError::Journal))
                }
            }
            Err(error) => owner_result(Err(error)),
        }
    }
}

pub(in crate::play::session) fn passive_ground_wander_speed(entity: &SpawnEntity) -> f64 {
    entity
        .attributes
        .base(&AttributeKind::MovementSpeed)
        .unwrap_or(0.2)
        * 10.0
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

fn build_herd_spawn_candidates(
    chunk: (i32, i32),
    spawns: &[HerdSpawn],
    player_positions: &[Vec3],
    lifecycle_tick: u64,
    minimum_player_distance: f64,
) -> Vec<SpawnEntity> {
    let mut passive_count = 0_usize;
    let mut hostile_count = 0_usize;
    let mut entities = Vec::new();
    for spawn in spawns {
        debug_assert_eq!(spawn.chunk, chunk);
        if spawn.hostile {
            if hostile_count >= MAX_HOSTILE_SPAWNS_PER_CHUNK {
                continue;
            }
        } else if passive_count >= MAX_PASSIVE_SPAWNS_PER_CHUNK {
            continue;
        }
        if !spawn_far_enough_from_players(player_positions, spawn.position, minimum_player_distance)
        {
            continue;
        }
        let mut entity = SpawnEntity::new(
            spawn.entity_type_id,
            spawn.entity_type_name.clone(),
            spawn.position,
        );
        entity.retained.spawn_tick = lifecycle_tick;
        entity.uuid = Some(herd_uuid(spawn.chunk, spawn.slot));
        apply_entity_facts(&mut entity);
        if let Some(color) = spawn.sheep_color {
            debug_assert_eq!(entity.type_name, "minecraft:sheep");
            entity.animal = Some(mc_entity::AnimalBreedingState::adult_sheep(color));
        }
        let ground_wander_speed = passive_ground_wander_speed(&entity);
        entity.goal = if spawn.hostile {
            GoalState::Wander {
                speed: HOSTILE_WANDER_SPEED,
                period_ticks: 20,
            }
        } else if entity_type_uses_aquatic_physics(&entity.type_name) {
            entity.on_ground = false;
            GoalState::AquaticWander {
                speed: PASSIVE_WANDER_SPEED * 0.9,
                vertical_speed: 0.18,
                period_ticks: 45,
            }
        } else {
            GoalState::Wander {
                speed: ground_wander_speed,
                period_ticks: 80,
            }
        };
        entities.push(entity);
        if spawn.hostile {
            hostile_count += 1;
        } else {
            passive_count += 1;
        }
    }
    entities
}

pub(super) fn spawn_far_enough_from_players(
    player_positions: &[Vec3],
    position: Vec3,
    minimum_distance: f64,
) -> bool {
    let min_distance_sq = minimum_distance * minimum_distance;
    player_positions
        .iter()
        .all(|player| distance_sq(position, *player) > min_distance_sq)
}

pub(in crate::play::session) fn install_committed_herd_spawns_locked(
    inner: &mut SessionRegistryInner,
    committed: Vec<EntitySnapshot>,
    _lifecycle_tick: u64,
) -> Vec<VisibilityDispatch> {
    let mut snapshots = Vec::with_capacity(committed.len());
    for entity in committed {
        if is_hostile_entity(&entity.type_name) {
            inner.hostile_entities.insert(entity.id);
            inner.natural_hostile_mobs.insert(entity.id);
        } else if entity_type_uses_aquatic_physics(&entity.type_name) {
            inner.natural_aquatic_mobs.insert(entity.id);
        } else {
            inner.natural_ground_mobs.insert(entity.id);
        }
        let aabb = entity_aabb(&entity.type_name);
        let snapshot = server_entity_snapshot_from(entity);
        inner
            .entity_type_aabbs
            .entry(snapshot.type_id)
            .or_insert(aabb);
        track_entity_chunk_locked(inner, snapshot.id, snapshot.position);
        initialize_entity_wire_state_from_snapshot_locked(inner, &snapshot);
        snapshots.push(snapshot);
    }
    install_committed_entity_publications_locked(inner, snapshots)
}
