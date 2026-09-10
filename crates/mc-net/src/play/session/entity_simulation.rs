use crate::play::simulation::RegionallyCommittedEntityMovement;
use mc_entity::{EntitySimulationResult, EntityTrackingMotion};

use super::entity_lifecycle::{
    remove_server_entity_locked, schedule_entity_death_locked, track_entity_chunk_locked,
    update_breeding_tick_tracking_locked,
};
use super::entity_physics_class::entity_type_uses_aquatic_physics;
use super::explosion_authority::schedule_primed_tnt_deadline_locked;
use super::interaction_geometry::{distance_sq, entity_aabb, entity_is_near_player_chunk};
use super::pickups::merge_item_entities_locked;
use super::simulation_input_publication::ExpectedEntityRoutingMove;
use super::visibility::{
    LastSentEntityState, entity_wire_move_for_kind, packed_head_yaw_changed,
    publish_server_entity_motion_locked,
    refresh_entity_target_visibility_with_old_observers_locked,
};
use super::*;

mod persistence_projection;

use persistence_projection::{EntityPersistenceMetadata, project_owner_save};

#[derive(Debug, Default)]
pub(crate) struct VillagerPopulationSelection {
    pub(super) candidates: HashSet<EntityId>,
    /// Exact active-simulation villager set returned by the owner lanes.
    pub(super) covered: HashSet<EntityId>,
    pub(super) proximity_seeds: Vec<Vec3>,
}

#[cfg(test)]
std::thread_local! {
    static MOVEMENT_VISIBILITY_INDEX_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static MOVEMENT_VISIBILITY_INDEX_EDGE_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static MOVEMENT_EXHAUSTIVE_MEMBERSHIP_CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct MovementFanoutWork {
    pub(super) index_builds: usize,
    pub(super) index_edge_visits: usize,
    pub(super) exhaustive_membership_checks: usize,
}

#[cfg(test)]
fn test_entity_projectile_physics_facts(
    steps: &[EntityPhysicsStep],
) -> EntityProjectilePhysicsFacts {
    EntityProjectilePhysicsFacts {
        arrows: test_arrow_physics_facts(steps),
        hurting: Vec::new(),
        throwable: Vec::new(),
    }
}

#[cfg(test)]
fn test_arrow_physics_facts(steps: &[EntityPhysicsStep]) -> Vec<ArrowPhysicsFact> {
    steps
        .iter()
        .map(|step| {
            let embedded_in_block = step.on_ground && step.velocity == mc_entity::Vec3::ZERO;
            ArrowPhysicsFact {
                arrow_id: step.id,
                block_hit: None,
                embedded_in_block,
                current_block_state: mc_world::BlockStateId(u32::from(embedded_in_block)),
                should_fall: !embedded_in_block,
                fall_velocity_scale: mc_entity::Vec3::new(0.1, 0.1, 0.1),
                in_water: false,
                in_water_or_rain: false,
            }
        })
        .collect()
}

#[cfg(test)]
pub(super) fn reset_movement_fanout_work() {
    MOVEMENT_VISIBILITY_INDEX_BUILDS.set(0);
    MOVEMENT_VISIBILITY_INDEX_EDGE_VISITS.set(0);
    MOVEMENT_EXHAUSTIVE_MEMBERSHIP_CHECKS.set(0);
}

#[cfg(test)]
pub(super) fn take_movement_fanout_work() -> MovementFanoutWork {
    MovementFanoutWork {
        index_builds: MOVEMENT_VISIBILITY_INDEX_BUILDS.replace(0),
        index_edge_visits: MOVEMENT_VISIBILITY_INDEX_EDGE_VISITS.replace(0),
        exhaustive_membership_checks: MOVEMENT_EXHAUSTIVE_MEMBERSHIP_CHECKS.replace(0),
    }
}

#[cfg(test)]
fn record_movement_visibility_index_build() {
    MOVEMENT_VISIBILITY_INDEX_BUILDS.set(MOVEMENT_VISIBILITY_INDEX_BUILDS.get() + 1);
}

#[cfg(test)]
fn record_movement_visibility_index_edge_visit() {
    MOVEMENT_VISIBILITY_INDEX_EDGE_VISITS.set(MOVEMENT_VISIBILITY_INDEX_EDGE_VISITS.get() + 1);
}

#[cfg(test)]
fn record_movement_exhaustive_membership_check() {
    MOVEMENT_EXHAUSTIVE_MEMBERSHIP_CHECKS.set(MOVEMENT_EXHAUSTIVE_MEMBERSHIP_CHECKS.get() + 1);
}

const VILLAGER_BRAIN_TICK_INTERVAL: u64 = 20;

const VILLAGER_GOSSIP_REACH_SQUARED: f64 = 5.0;
const VILLAGER_GOSSIP_COOLDOWN_TICKS: u64 = 1_200;
const VILLAGER_GOSSIP_CELL_SIZE: f64 = 3.0;

#[derive(Clone, Copy)]
struct VillagerProfessionContext<'a> {
    world_read: &'a mc_world::WorldReadView,
    blocks: &'a mc_world::BlockRegistry,
    items: &'a mc_data::items::ItemRegistry,
}

fn current_villager_brain(
    entity: &EntitySnapshot,
) -> Option<mc_entity::villager_26_1_2::VillagerBrainState> {
    let villager = entity.retained.villager?;
    Some(entity.retained.villager_brain.clone().unwrap_or_else(|| {
        mc_entity::villager_26_1_2::VillagerBrainState::adult(
            mc_entity::villager_26_1_2::default_villager_pois(entity.position, villager.profession),
        )
    }))
}

fn villager_job_site_block_pos(position: Vec3) -> mc_world::BlockPos {
    mc_world::BlockPos {
        x: position.x.floor() as i32,
        y: position.y.floor() as i32,
        z: position.z.floor() as i32,
    }
}

fn supported_profession_offer(
    entity: &mc_entity::EntitySimulationProjection,
    context: VillagerProfessionContext<'_>,
    cached_merchant: Option<&mc_entity::villager_merchant_26_1_2::VillagerMerchantState>,
) -> Option<mc_entity::RegionalVillagerProfessionOffer> {
    let job_site = entity.villager_job_site?;
    let state = context
        .world_read
        .get_cached_block(villager_job_site_block_pos(job_site))?;
    let block = &context.blocks.by_id(state)?.block.id;
    match mc_data::villager_trades_26_1_2::supported_profession_for_job_site_26_1_2(block)? {
        "toolsmith" => Some(mc_entity::RegionalVillagerProfessionOffer {
            profession: mc_entity::VillagerProfession::Toolsmith,
            merchant: cached_merchant?.clone(),
        }),
        _ => None,
    }
}

fn regional_profession_offers_by_block_state(
    context: VillagerProfessionContext<'_>,
    cached_merchant: Option<&mc_entity::villager_merchant_26_1_2::VillagerMerchantState>,
) -> HashMap<u32, mc_entity::RegionalVillagerProfessionOffer> {
    let smithing_table =
        mc_data::Identifier::parse("minecraft:smithing_table").expect("static identifier");
    let Some(block) = context.blocks.block(&smithing_table) else {
        return HashMap::new();
    };
    let Some(merchant) = cached_merchant else {
        return HashMap::new();
    };
    block
        .states
        .iter()
        .map(|state| {
            (
                state.0,
                mc_entity::RegionalVillagerProfessionOffer {
                    profession: mc_entity::VillagerProfession::Toolsmith,
                    merchant: merchant.clone(),
                },
            )
        })
        .collect()
}

fn villager_schedule_boundary(
    profile: &mc_entity::villager_26_1_2::VillagerBrainProfile,
    schedule: mc_entity::villager_26_1_2::VillagerScheduleKind,
    day_time: i64,
) -> bool {
    let entries = match schedule {
        mc_entity::villager_26_1_2::VillagerScheduleKind::Adult => &profile.adult_schedule,
        mc_entity::villager_26_1_2::VillagerScheduleKind::Baby => &profile.baby_schedule,
    };
    let normalized = day_time.rem_euclid(24_000);
    entries.iter().any(|entry| entry.day_time == normalized)
}

fn any_villager_schedule_boundary(
    profile: &mc_entity::villager_26_1_2::VillagerBrainProfile,
    day_time: i64,
) -> bool {
    villager_schedule_boundary(
        profile,
        mc_entity::villager_26_1_2::VillagerScheduleKind::Adult,
        day_time,
    ) || villager_schedule_boundary(
        profile,
        mc_entity::villager_26_1_2::VillagerScheduleKind::Baby,
        day_time,
    )
}

fn villager_brain_phase_due(entity: EntityId, lifecycle_tick: u64) -> bool {
    lifecycle_tick
        .wrapping_add(u64::from(entity.0.unsigned_abs()))
        .is_multiple_of(VILLAGER_BRAIN_TICK_INTERVAL)
}

/// Re-plan cadence for far-periphery goals: entities farther than
/// [`NEAR_GOAL_CHUNK_RADIUS`] (Chebyshev chunks) from every player chunk
/// re-plan once every [`FAR_GOAL_CADENCE_TICKS`], phase-rotated by entity id
/// so roughly one quarter is due each tick. Near entities and entities with
/// unknown chunks plan every tick (fail open), as does the whole population
/// when no player positions are published yet. Deferred entities keep moving
/// every tick via current-simulation-results continuation at the apply site.
const FAR_GOAL_CADENCE_TICKS: u64 = 4;
const NEAR_GOAL_CHUNK_RADIUS: i32 = 1;

fn far_goal_tick_due(entity: EntityId, tick: u64) -> bool {
    tick.wrapping_add(u64::from(entity.0.unsigned_abs()))
        .is_multiple_of(FAR_GOAL_CADENCE_TICKS)
}

pub(super) fn split_goal_population_by_distance(
    population: &HashSet<EntityId>,
    entity_chunk: impl Fn(EntityId) -> Option<(i32, i32)>,
    player_chunks: &[(i32, i32)],
    tick: u64,
) -> (HashSet<EntityId>, HashSet<EntityId>) {
    if player_chunks.is_empty() {
        return (population.clone(), HashSet::new());
    }
    let mut planned = HashSet::new();
    let mut deferred = HashSet::new();
    for entity in population.iter().copied() {
        let near = entity_chunk(entity).is_none_or(|(cx, cz)| {
            player_chunks.iter().any(|&(px, pz)| {
                cx.abs_diff(px) <= NEAR_GOAL_CHUNK_RADIUS as u32
                    && cz.abs_diff(pz) <= NEAR_GOAL_CHUNK_RADIUS as u32
            })
        });
        if near || far_goal_tick_due(entity, tick) {
            planned.insert(entity);
        } else {
            deferred.insert(entity);
        }
    }
    (planned, deferred)
}

pub(super) fn villager_brain_probe_ids(
    active_population: &HashSet<EntityId>,
    overridden_villagers: &HashSet<EntityId>,
    lifecycle_tick: u64,
    day_time: i64,
    profile: &mc_entity::villager_26_1_2::VillagerBrainProfile,
) -> HashSet<EntityId> {
    let mut due = if any_villager_schedule_boundary(profile, day_time) {
        active_population.clone()
    } else {
        active_population
            .iter()
            .copied()
            .filter(|entity| villager_brain_phase_due(*entity, lifecycle_tick))
            .collect()
    };
    due.extend(
        overridden_villagers
            .iter()
            .copied()
            .filter(|entity| active_population.contains(entity)),
    );
    due
}

fn villager_gossip_activity_allows_transfer(
    activity: mc_entity::villager_26_1_2::VillagerActivity,
) -> bool {
    matches!(
        activity,
        mc_entity::villager_26_1_2::VillagerActivity::Idle
            | mc_entity::villager_26_1_2::VillagerActivity::Meet
    )
}

fn villager_gossip_cooldown_ready(timestamp: u64, last_gossip_time: u64) -> bool {
    timestamp < last_gossip_time
        || timestamp >= last_gossip_time.saturating_add(VILLAGER_GOSSIP_COOLDOWN_TICKS)
}

fn villager_gossip_cell(position: Vec3) -> (i32, i32, i32) {
    (
        (position.x / VILLAGER_GOSSIP_CELL_SIZE).floor() as i32,
        (position.y / VILLAGER_GOSSIP_CELL_SIZE).floor() as i32,
        (position.z / VILLAGER_GOSSIP_CELL_SIZE).floor() as i32,
    )
}

// Solaris does not yet own vanilla's complete per-entity RandomSource stream.
// Mix stable actor facts into a deterministic seed, while the transfer container
// itself uses Java's exact legacy nextInt(bound) algorithm.
fn villager_gossip_seed(receiver: uuid::Uuid, source: uuid::Uuid, timestamp: u64) -> u64 {
    let receiver = receiver.as_u128();
    let source = source.as_u128();
    splitmix64(
        (receiver as u64)
            ^ ((receiver >> 64) as u64).rotate_left(11)
            ^ (source as u64).rotate_left(23)
            ^ ((source >> 64) as u64).rotate_left(37)
            ^ timestamp.wrapping_mul(0x9E37_79B9_7F4A_7C15),
    )
}

fn select_villager_gossip_target(
    receiver: &EntitySnapshot,
    receiver_brain: &mc_entity::villager_26_1_2::VillagerBrainState,
    candidates: &HashMap<EntityId, EntitySnapshot>,
    cells: &HashMap<(i32, i32, i32), Vec<EntityId>>,
    reserved: &HashSet<EntityId>,
    timestamp: u64,
    cross_region_only: bool,
) -> Option<EntityId> {
    let eligible = |target: EntityId| {
        if target == receiver.id || reserved.contains(&target) {
            return None;
        }
        let snapshot = candidates.get(&target)?;
        if cross_region_only
            && mc_entity::RegionKey::from_position(snapshot.position)
                == mc_entity::RegionKey::from_position(receiver.position)
        {
            return None;
        }
        let brain = current_villager_brain(snapshot)?;
        if !villager_gossip_cooldown_ready(timestamp, brain.last_gossip_time) {
            return None;
        }
        let distance = distance_sq(receiver.position, snapshot.position);
        (distance <= VILLAGER_GOSSIP_REACH_SQUARED).then_some((target, distance))
    };

    if let Some(target) = receiver_brain.interaction_target
        && eligible(target).is_some()
    {
        return Some(target);
    }

    let (cell_x, cell_y, cell_z) = villager_gossip_cell(receiver.position);
    let mut best = None::<(EntityId, f64)>;
    for x in (cell_x - 1)..=(cell_x + 1) {
        for y in (cell_y - 1)..=(cell_y + 1) {
            for z in (cell_z - 1)..=(cell_z + 1) {
                let Some(ids) = cells.get(&(x, y, z)) else {
                    continue;
                };
                for &target in ids {
                    let Some((target, distance)) = eligible(target) else {
                        continue;
                    };
                    if best.is_none_or(|(best_id, best_distance)| {
                        distance < best_distance || distance == best_distance && target < best_id
                    }) {
                        best = Some((target, distance));
                    }
                }
            }
        }
    }
    best.map(|(target, _)| target)
}

pub(super) fn commit_villager_gossip_transfer_pair(
    entities: &mut EntityStoreGuard<'_>,
    receiver: EntitySnapshot,
    source: EntitySnapshot,
    timestamp: u64,
) -> bool {
    if receiver.id == source.id
        || receiver.lifecycle != EntityLifecycle::Alive
        || source.lifecycle != EntityLifecycle::Alive
        || receiver.type_name != "minecraft:villager"
        || source.type_name != "minecraft:villager"
        || distance_sq(receiver.position, source.position) > VILLAGER_GOSSIP_REACH_SQUARED
    {
        return false;
    }
    let Some(mut receiver_brain) = current_villager_brain(&receiver) else {
        return false;
    };
    let Some(mut source_brain) = current_villager_brain(&source) else {
        return false;
    };
    if !villager_gossip_activity_allows_transfer(receiver_brain.activity)
        || !villager_gossip_cooldown_ready(timestamp, receiver_brain.last_gossip_time)
        || !villager_gossip_cooldown_ready(timestamp, source_brain.last_gossip_time)
    {
        return false;
    }

    let source_gossip = source.retained.villager_gossip.clone().unwrap_or_default();
    let mut receiver_gossip = receiver
        .retained
        .villager_gossip
        .clone()
        .unwrap_or_default();
    let Ok(gossip_changed) = receiver_gossip.transfer_from_seeded(
        &source_gossip,
        villager_gossip_seed(receiver.uuid, source.uuid, timestamp),
        mc_entity::villager_gossip_26_1_2::MAX_TRANSFER_COUNT,
    ) else {
        return false;
    };

    receiver_brain.interaction_target = Some(source.id);
    receiver_brain.last_gossip_time = timestamp;
    source_brain.last_gossip_time = timestamp;
    let mut receiver_next = receiver.clone();
    receiver_next.retained.villager_brain = Some(receiver_brain);
    if gossip_changed || receiver.retained.villager_gossip.is_some() {
        receiver_next.retained.villager_gossip = Some(receiver_gossip);
    }
    let mut source_next = source.clone();
    source_next.retained.villager_brain = Some(source_brain);

    entities.replace_snapshots_if_current([(receiver, receiver_next), (source, source_next)])
}

fn apply_cross_region_villager_gossip_transfers(
    entities: &mut EntityStoreGuard<'_>,
    initiator_ids: &HashSet<EntityId>,
    candidate_ids: &HashSet<EntityId>,
    timestamp: u64,
) -> usize {
    apply_villager_gossip_transfers_inner(entities, initiator_ids, candidate_ids, timestamp, true)
}

fn apply_villager_gossip_transfers_inner(
    entities: &mut EntityStoreGuard<'_>,
    initiator_ids: &HashSet<EntityId>,
    candidate_ids: &HashSet<EntityId>,
    timestamp: u64,
    cross_region_only: bool,
) -> usize {
    if initiator_ids.is_empty() || candidate_ids.len() < 2 {
        return 0;
    }
    let mut candidates = HashMap::<EntityId, EntitySnapshot>::new();
    let mut cells = HashMap::<(i32, i32, i32), Vec<EntityId>>::new();
    for snapshot in entities.snapshots_for_ids_uncached(candidate_ids) {
        let id = snapshot.id;
        if snapshot.lifecycle != EntityLifecycle::Alive
            || snapshot.type_name != "minecraft:villager"
            || snapshot.retained.villager.is_none()
        {
            continue;
        }
        cells
            .entry(villager_gossip_cell(snapshot.position))
            .or_default()
            .push(id);
        candidates.insert(id, snapshot);
    }

    let mut ordered_initiators = initiator_ids.iter().copied().collect::<Vec<_>>();
    ordered_initiators.sort_unstable();
    let mut reserved = HashSet::new();
    let mut applied = 0;
    for receiver_id in ordered_initiators {
        if reserved.contains(&receiver_id) {
            continue;
        }
        let Some(receiver) = candidates.get(&receiver_id).cloned() else {
            continue;
        };
        let Some(receiver_brain) = current_villager_brain(&receiver) else {
            continue;
        };
        if !villager_gossip_activity_allows_transfer(receiver_brain.activity)
            || !villager_gossip_cooldown_ready(timestamp, receiver_brain.last_gossip_time)
        {
            continue;
        }
        let Some(source_id) = select_villager_gossip_target(
            &receiver,
            &receiver_brain,
            &candidates,
            &cells,
            &reserved,
            timestamp,
            cross_region_only,
        ) else {
            continue;
        };
        let Some(source) = candidates.get(&source_id).cloned() else {
            continue;
        };
        if commit_villager_gossip_transfer_pair(entities, receiver, source, timestamp) {
            reserved.insert(receiver_id);
            reserved.insert(source_id);
            applied += 1;
        }
    }
    applied
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

// load-bench-only coarse phase profiler for apply_entity_physics_and_dispatch_core.
// Emits one PHYSICS_APPLY_PROFILE line per large apply (input steps > 256) when the
// profiler guard drops at function return (covers every return path).
#[cfg(feature = "load-bench")]
struct PhysicsApplyProfile {
    tick: u64,
    total_started: std::time::Instant,
    phase_started: std::time::Instant,
    publication_refresh_started: Option<std::time::Instant>,
    movement_plan_started: bool,
    preflight_lock_us: u64,
    preflight_prefetch_us: u64,
    prefetch_missing: usize,
    fenced_apply: bool,
    motion_lookup_calls: usize,
    motion_lookup_samples: usize,
    motion_lookup_sample_us: u64,
    preflight_capture_us: u64,
    preflight_finalize_us: u64,
    preflight_us: u64,
    owner_apply_us: u64,
    publication_refresh_us: u64,
    locked_publish_us: u64,
    input_steps: usize,
    filtered_steps: usize,
    effective_steps: usize,
    applied_kinematics: usize,
    applied_steps: usize,
    tracker_inputs: usize,
    movements: usize,
    chunk_crossings: usize,
    publication_retries: u64,
}

#[cfg(feature = "load-bench")]
impl PhysicsApplyProfile {
    fn new(tick: u64, input_steps: usize) -> Self {
        let now = std::time::Instant::now();
        Self {
            tick,
            total_started: now,
            phase_started: now,
            movement_plan_started: false,
            publication_refresh_started: None,
            preflight_lock_us: 0,
            preflight_prefetch_us: 0,
            preflight_capture_us: 0,
            preflight_finalize_us: 0,
            prefetch_missing: 0,
            motion_lookup_calls: 0,
            motion_lookup_samples: 0,
            motion_lookup_sample_us: 0,
            fenced_apply: false,
            preflight_us: 0,
            owner_apply_us: 0,
            publication_refresh_us: 0,
            locked_publish_us: 0,
            input_steps,
            filtered_steps: input_steps,
            effective_steps: input_steps,
            applied_kinematics: 0,
            applied_steps: 0,
            tracker_inputs: 0,
            movements: 0,
            chunk_crossings: 0,
            publication_retries: 0,
        }
    }

    fn elapsed_us(started: std::time::Instant) -> u64 {
        u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
    }

    fn finish_preflight(&mut self) {
        self.preflight_us = Self::elapsed_us(self.phase_started);
        self.phase_started = std::time::Instant::now();
    }

    fn finish_owner_apply(&mut self, applied_kinematics: usize, fenced_apply: bool) {
        self.owner_apply_us = Self::elapsed_us(self.phase_started);
        self.applied_kinematics = applied_kinematics;
        self.fenced_apply = fenced_apply;
    }

    fn begin_publication_refresh(&mut self) {
        self.publication_refresh_started = Some(std::time::Instant::now());
    }

    fn finish_publication_refresh(&mut self) {
        if let Some(started) = self.publication_refresh_started.take() {
            self.publication_refresh_us = self
                .publication_refresh_us
                .saturating_add(Self::elapsed_us(started));
        }
    }

    fn record_publication_retry(&mut self) {
        self.publication_retries = self.publication_retries.saturating_add(1);
    }

    fn begin_locked_publish(&mut self) {
        self.phase_started = std::time::Instant::now();
    }

    fn finish_locked_publish(&mut self) {
        self.locked_publish_us = Self::elapsed_us(self.phase_started);
        self.phase_started = std::time::Instant::now();
        self.movement_plan_started = true;
    }

    fn record_filtered(&mut self, filtered_steps: usize) {
        self.filtered_steps = filtered_steps;
    }

    fn record_effective(&mut self, effective_steps: usize) {
        self.effective_steps = effective_steps;
    }

    fn record_applied_steps(&mut self, applied_steps: usize) {
        self.applied_steps = applied_steps;
    }

    fn record_tracker_inputs(&mut self, tracker_inputs: usize) {
        self.tracker_inputs = tracker_inputs;
    }

    fn record_chunk_crossings(&mut self, chunk_crossings: usize) {
        self.chunk_crossings = chunk_crossings;
    }

    fn record_movements(&mut self, movements: usize) {
        self.movements = movements;
    }
}

#[cfg(feature = "load-bench")]
impl Drop for PhysicsApplyProfile {
    fn drop(&mut self) {
        if self.input_steps <= 256 {
            return;
        }
        let movement_plan_us = if self.movement_plan_started {
            Self::elapsed_us(self.phase_started)
        } else {
            0
        };
        let total_us = Self::elapsed_us(self.total_started);
        let motion_lookup_est_us = if self.motion_lookup_samples == 0 {
            0
        } else {
            self.motion_lookup_sample_us
                .saturating_mul(self.motion_lookup_calls as u64)
                / self.motion_lookup_samples as u64
        };
        tracing::trace!(
            "PHYSICS_APPLY_PROFILE tick={} total_us={total_us} preflight_us={} preflight_lock_us={} preflight_prefetch_us={} prefetch_missing={} fenced_apply={} preflight_capture_us={} motion_lookup_calls={} motion_lookup_samples={} motion_lookup_sample_us={} motion_lookup_est_us={} preflight_finalize_us={} owner_apply_us={} publication_refresh_us={} locked_publish_us={} movement_plan_us={} input_steps={} filtered={} effective={} applied_kinematics={} applied_steps={} tracker_inputs={} movements={} chunk_crossings={} publication_retries={}",
            self.tick,
            self.preflight_us,
            self.preflight_lock_us,
            self.preflight_prefetch_us,
            self.prefetch_missing,
            self.fenced_apply,
            self.preflight_capture_us,
            self.motion_lookup_calls,
            self.motion_lookup_samples,
            self.motion_lookup_sample_us,
            motion_lookup_est_us,
            self.preflight_finalize_us,
            self.owner_apply_us,
            self.publication_refresh_us,
            self.locked_publish_us,
            movement_plan_us,
            self.input_steps,
            self.filtered_steps,
            self.effective_steps,
            self.applied_kinematics,
            self.applied_steps,
            self.tracker_inputs,
            self.movements,
            self.chunk_crossings,
            self.publication_retries,
        );
    }
}

impl SessionRegistry {
    fn publish_villager_metadata_updates(&self, expected: Vec<EntitySnapshot>) {
        if expected.is_empty() {
            return;
        }
        let current = self.current_expected_entity_snapshots(expected);
        if current.is_empty() {
            return;
        }
        let mut inner = self.lock_inner("publish villager profession metadata");
        let mut dispatches = Vec::new();
        for entity in current {
            let projected = server_entity_snapshot_from(entity);
            let entity_id = projected.id;
            let changed = inner
                .published_entity_snapshots
                .get(&entity_id)
                .is_none_or(|published| published.villager != projected.villager);
            let Some(published) = inner.published_entity_snapshots.get_mut(&entity_id) else {
                continue;
            };
            published.villager = projected.villager;
            if !changed {
                continue;
            }
            let recipients =
                session_recipients(&inner, visible_entity_observers_locked(&inner, entity_id));
            dispatches.extend(visibility_dispatches(recipients, || {
                OutboundCommand::UpdateEntityData(projected.clone())
            }));
        }
        record_entity_dispatches_locked(&mut inner, &dispatches);
        drop(inner);
        dispatch_visibility_commands(dispatches);
    }

    pub(crate) fn entity_update_budget_observation(&self) -> (usize, usize, usize, usize) {
        (
            self.entity_update_budget_per_lane.load(Ordering::Relaxed),
            self.entity_update_budget_total.load(Ordering::Relaxed),
            self.entity_update_selected.load(Ordering::Relaxed),
            self.entity_update_active_population.load(Ordering::Relaxed),
        )
    }

    pub(crate) fn set_entity_movement_publication_budget(&self, budget: usize) {
        self.entity_movement_publication_budget
            .store(budget.max(1), Ordering::Relaxed);
    }

    pub(crate) fn entity_movement_publication_budget(&self) -> usize {
        self.entity_movement_publication_budget
            .load(Ordering::Relaxed)
            .max(1)
    }

    pub(crate) fn tick_entities_and_collect_physics_queries_regional(
        &self,
        cpu_resources: &crate::chunk_pipeline::ChunkPipelineResources,
        tick: u64,
        policy: EntitySimulationTickPolicy,
        world: EntitySimulationWorldContext<'_>,
    ) -> (
        Vec<EntityPhysicsQuery>,
        Option<mc_entity::VersionedEntityKinematics>,
        VillagerPopulationSelection,
        Option<mc_entity::RegionalPreparedEntityPhysics>,
    ) {
        self.tick_entities_and_collect_physics_queries_core(
            Some(cpu_resources),
            tick,
            policy,
            world.pathing(),
            world.profession_context(),
            world.regional_pathing_materials(),
        )
    }

    #[cfg(test)]
    pub(crate) fn tick_entities_and_collect_physics_queries(
        &self,
        tick: u64,
    ) -> Vec<EntityPhysicsQuery> {
        self.tick_entities_and_collect_physics_queries_core(
            None,
            tick,
            EntitySimulationTickPolicy {
                pathing_candidates_per_entity: PathingBudget::DEFAULT.max_candidates_per_entity,
                simulation_distance: DEFAULT_VIEW_DISTANCE,
            },
            None,
            None,
            None,
        )
        .0
    }

    #[cfg(test)]
    pub(crate) fn tick_entities_and_collect_physics_queries_with_simulation_distance(
        &self,
        tick: u64,
        simulation_distance: i32,
    ) -> Vec<EntityPhysicsQuery> {
        self.tick_entities_and_collect_physics_queries_core(
            None,
            tick,
            EntitySimulationTickPolicy {
                pathing_candidates_per_entity: PathingBudget::DEFAULT.max_candidates_per_entity,
                simulation_distance,
            },
            None,
            None,
            None,
        )
        .0
    }

    #[cfg(test)]
    pub(crate) fn tick_entities_and_collect_physics_queries_with_pathing_budget(
        &self,
        tick: u64,
        pathing_candidates_per_entity: usize,
    ) -> Vec<EntityPhysicsQuery> {
        self.tick_entities_and_collect_physics_queries_core(
            None,
            tick,
            EntitySimulationTickPolicy {
                pathing_candidates_per_entity,
                simulation_distance: DEFAULT_VIEW_DISTANCE,
            },
            None,
            None,
            None,
        )
        .0
    }

    #[cfg(test)]
    pub(crate) fn tick_entities_and_collect_physics_queries_with_terrain(
        &self,
        tick: u64,
        world_read: &mc_world::WorldReadView,
        pathing_materials: &mc_physics::BlockMaterialIds,
    ) -> Vec<EntityPhysicsQuery> {
        self.tick_entities_and_collect_physics_queries_core(
            None,
            tick,
            EntitySimulationTickPolicy {
                pathing_candidates_per_entity: PathingBudget::DEFAULT.max_candidates_per_entity,
                simulation_distance: DEFAULT_VIEW_DISTANCE,
            },
            Some((world_read, pathing_materials)),
            None,
            None,
        )
        .0
    }

    #[cfg(test)]
    pub(crate) fn tick_entities_and_collect_physics_queries_with_profession_context(
        &self,
        tick: u64,
        world_read: &mc_world::WorldReadView,
        blocks: &mc_world::BlockRegistry,
        items: &mc_data::items::ItemRegistry,
    ) -> Vec<EntityPhysicsQuery> {
        self.tick_entities_and_collect_physics_queries_core(
            None,
            tick,
            EntitySimulationTickPolicy {
                pathing_candidates_per_entity: PathingBudget::DEFAULT.max_candidates_per_entity,
                simulation_distance: DEFAULT_VIEW_DISTANCE,
            },
            None,
            Some((world_read, blocks, items)),
            None,
        )
        .0
    }

    fn tick_entities_and_collect_physics_queries_core(
        &self,
        cpu_resources: Option<&crate::chunk_pipeline::ChunkPipelineResources>,
        tick: u64,
        policy: EntitySimulationTickPolicy,
        pathing: Option<(&mc_world::WorldReadView, &mc_physics::BlockMaterialIds)>,
        profession_context: Option<(
            &mc_world::WorldReadView,
            &mc_world::BlockRegistry,
            &mc_data::items::ItemRegistry,
        )>,
        regional_pathing_materials: Option<Arc<mc_physics::BlockMaterialIds>>,
    ) -> (
        Vec<EntityPhysicsQuery>,
        Option<mc_entity::VersionedEntityKinematics>,
        VillagerPopulationSelection,
        Option<mc_entity::RegionalPreparedEntityPhysics>,
    ) {
        let EntitySimulationTickPolicy {
            pathing_candidates_per_entity,
            simulation_distance,
        } = policy;
        #[cfg(feature = "load-bench")]
        let goal_profile_started = std::time::Instant::now();
        if !self.has_live_sessions() {
            self.clear_active_simulation_selection();
            return (
                Vec::new(),
                None,
                VillagerPopulationSelection::default(),
                None,
            );
        }
        let live_session_generation = self.live_session_generation.load(Ordering::Acquire);
        let (world_read, pathing_materials) = pathing.unzip();
        let profession_context =
            profession_context.map(|(world_read, blocks, items)| VillagerProfessionContext {
                world_read,
                blocks,
                items,
            });
        let recipients = self.movement_recipients.load_full();
        let mut player_positions = Vec::new();
        let mut hostile_target_positions = Vec::new();
        let mut combat_targets_by_entity_id = HashMap::new();
        for publication in recipients.values() {
            let target = *publication.combat_target();
            let position = Vec3::new(target.pose().x, target.pose().y, target.pose().z);
            if target.is_alive() {
                player_positions.push(position);
            }
            if target.is_targetable() {
                hostile_target_positions.push(position);
                combat_targets_by_entity_id.insert(
                    publication.entity_id(),
                    Vec3::new(position.x, position.y + 0.9, position.z),
                );
            }
        }
        let terrain_pathing_entities = self.simulation_inputs.terrain_pathing_entities();
        let villager_day_time = i64::try_from(self.world_time()).unwrap_or(i64::MAX);
        let villager_profile = self.villager_brain_profile();
        let overridden_villagers = self.overridden_villager_entities();
        let hostile_target_positions = Arc::<[Vec3]>::from(hostile_target_positions);
        let combat_targets_by_entity_id = Arc::new(combat_targets_by_entity_id);
        // Merchant offers are rebuilt from the item registry; do it once per
        // tick outside the entity-store lock instead of once per villager
        // while holding it.
        let cached_toolsmith_merchant =
            profession_context.and_then(|context| toolsmith_merchant_state(context.items));
        if self.live_session_generation.load(Ordering::Acquire) != live_session_generation {
            self.clear_active_simulation_selection();
            return (
                Vec::new(),
                None,
                VillagerPopulationSelection::default(),
                None,
            );
        }
        let mut regional_tick =
            world_read
                .zip(regional_pathing_materials)
                .map(|(world_read, materials)| {
                    let (active_chunks, simulation_chunks) =
                        self.simulation_inputs.active_chunks_matching(|chunk| {
                            entity_is_near_player_chunk(
                                chunk,
                                &player_positions,
                                simulation_distance,
                            )
                        });
                    let snapshot_chunks = active_chunks
                        .iter()
                        .map(|&(x, z)| mc_world::ChunkPos { x, z })
                        .collect::<Vec<_>>();
                    let pathing_snapshot = Arc::new(world_read.snapshot_chunks(&snapshot_chunks));
                    let profession_offers_by_block_state = Arc::new(
                        profession_context
                            .map(|context| {
                                regional_profession_offers_by_block_state(
                                    context,
                                    cached_toolsmith_merchant.as_ref(),
                                )
                            })
                            .unwrap_or_default(),
                    );
                    let world = Arc::new(mc_entity::RegionalTickWorld::new(
                        Arc::clone(&active_chunks),
                        Arc::clone(&terrain_pathing_entities),
                        pathing_snapshot,
                        world_read.clone(),
                        materials,
                    ));
                    let output =
                        self.entities
                            .tick_owned_regions(mc_entity::RegionalEntityTickInput {
                                tick,
                                simulation_chunks: Arc::new(simulation_chunks.clone()),
                                goals: mc_entity::RegionalGoalTickInputs {
                                    active_chunks: Some(Arc::clone(&active_chunks)),
                                    terrain_pathing_entities: Arc::clone(&terrain_pathing_entities),
                                    hostile_target_positions: Some(Arc::clone(
                                        &hostile_target_positions,
                                    )),
                                    combat_targets: Arc::clone(&combat_targets_by_entity_id),
                                    villager: Some(Arc::new(
                                        mc_entity::RegionalVillagerGoalTickInputs {
                                            day_time: villager_day_time,
                                            profile: Arc::clone(&villager_profile),
                                            profession_offers: Arc::new(HashMap::new()),
                                        },
                                    )),
                                    mob_behaviors: self.mob_behavior_table(),
                                },
                                world,
                                pathing_budget: PathingBudget {
                                    max_candidates_per_entity: pathing_candidates_per_entity.max(1),
                                    ..PathingBudget::DEFAULT
                                },
                                profession_offers_by_block_state,
                            });
                    debug_assert!(
                        output
                            .active_hostile_ids
                            .iter()
                            .chain(&output.villager_population_candidates)
                            .chain(&output.villager_ids)
                            .chain(&output.villager_proximity_seeds)
                            .all(|(entity, _)| !output.fallback_entity_ids.contains(entity))
                    );
                    (active_chunks, simulation_chunks, output)
                });
        let (active_chunks, simulation_chunks, active_population_count, goal_population_ids) =
            if let Some((active_chunks, simulation_chunks, output)) = regional_tick.as_ref() {
                (
                    Arc::clone(active_chunks),
                    simulation_chunks.clone(),
                    output.active_entity_count,
                    output.fallback_entity_ids.clone(),
                )
            } else {
                let (active_chunks, simulation_chunks, active_population_ids) = self
                    .simulation_inputs
                    .active_entity_candidates_matching_chunks(|chunk| {
                        entity_is_near_player_chunk(chunk, &player_positions, simulation_distance)
                    });
                (
                    active_chunks,
                    simulation_chunks,
                    active_population_ids.len(),
                    active_population_ids,
                )
            };
        let goal_population_ids = Arc::new(goal_population_ids);
        let villager_brain_probe_ids = villager_brain_probe_ids(
            &goal_population_ids,
            &overridden_villagers,
            tick,
            villager_day_time,
            &villager_profile,
        );
        // Goal re-plan cadence is tiered, physics is not: near entities
        // re-plan every tick, far-periphery entities re-plan once every
        // FAR_GOAL_CADENCE_TICKS (phase-rotated by entity id, see
        // split_goal_population_by_distance). Every entity still moves every
        // tick: off-phase far entities reuse current simulation results at
        // the apply site below, so selection stays the full population and
        // only decision refresh rate drops with distance.
        let lane_count = cpu_resources.map_or(1, |resources| resources.cpu_capacity().max(1));
        self.entity_update_budget_per_lane.store(
            active_population_count.div_ceil(lane_count),
            Ordering::Relaxed,
        );
        self.entity_update_budget_total
            .store(active_population_count, Ordering::Relaxed);
        self.entity_update_selected
            .store(active_population_count, Ordering::Relaxed);
        self.entity_update_active_population
            .store(active_population_count, Ordering::Relaxed);
        #[cfg(feature = "load-bench")]
        let selection_us =
            u64::try_from(goal_profile_started.elapsed().as_micros()).unwrap_or(u64::MAX);
        #[cfg(feature = "load-bench")]
        let projection_started = std::time::Instant::now();
        if active_chunks.is_empty() {
            self.clear_active_simulation_selection();
            return (
                Vec::new(),
                None,
                VillagerPopulationSelection::default(),
                None,
            );
        }
        if goal_population_ids.is_empty() && regional_tick.is_some() {
            let (_, _, output) = regional_tick
                .take()
                .expect("empty fallback population requires a regional tick");
            return self.finish_entity_tick_selection(
                live_session_generation,
                simulation_chunks,
                output.active_entity_count,
                Vec::new(),
                None,
                false,
                HashSet::new(),
                Vec::new(),
                Some(output),
            );
        }
        let player_chunks = player_positions
            .iter()
            .map(|position| chunk_pos_from_coords(position.x, position.z))
            .collect::<Vec<_>>();
        let (planned_goal_ids, deferred_goal_ids) = split_goal_population_by_distance(
            &goal_population_ids,
            |entity| self.simulation_inputs.entity_chunk(entity),
            &player_chunks,
            tick,
        );
        let planned_goal_ids = Arc::new(planned_goal_ids);
        let mut entities = self.lock_entities("prepare entity goals");
        let active_villager_ids = villager_brain_probe_ids;
        let villager_profession_offers = profession_context.map_or_else(HashMap::new, |context| {
            entities
                .simulation_projections_for_ids(&active_villager_ids)
                .into_iter()
                .filter_map(|projection| {
                    supported_profession_offer(
                        &projection,
                        context,
                        cached_toolsmith_merchant.as_ref(),
                    )
                    .map(|offer| (projection.id, offer))
                })
                .collect::<HashMap<_, _>>()
        });
        let villager_profession_metadata = villager_profession_offers
            .iter()
            .map(|(entity, offer)| (*entity, offer.profession))
            .collect::<HashMap<_, _>>();
        #[cfg(feature = "load-bench")]
        let projection_us =
            u64::try_from(projection_started.elapsed().as_micros()).unwrap_or(u64::MAX);
        #[cfg(feature = "load-bench")]
        let targets_us = 0_u64;
        #[cfg(feature = "load-bench")]
        let prepare_started = std::time::Instant::now();
        let prepared_goal_tick = entities.prepare_goal_tick_with_pathing_for_ids_with_inputs(
            tick,
            Arc::clone(&planned_goal_ids),
            mc_entity::RegionalGoalTickInputs {
                active_chunks: Some(Arc::clone(&active_chunks)),
                terrain_pathing_entities: Arc::clone(&terrain_pathing_entities),
                hostile_target_positions: Some(Arc::clone(&hostile_target_positions)),
                combat_targets: Arc::clone(&combat_targets_by_entity_id),
                villager: Some(Arc::new(mc_entity::RegionalVillagerGoalTickInputs {
                    day_time: villager_day_time,
                    profile: Arc::clone(&villager_profile),
                    profession_offers: Arc::new(villager_profession_offers),
                })),
                mob_behaviors: self.mob_behavior_table(),
            },
        );
        let cross_region_villager_candidates = prepared_goal_tick
            .cross_region_villager_candidates()
            .clone();
        #[cfg(feature = "load-bench")]
        let goal_entity_count = prepared_goal_tick.goal_entity_count();
        #[cfg(feature = "load-bench")]
        let prepare_us = u64::try_from(prepare_started.elapsed().as_micros()).unwrap_or(u64::MAX);
        drop(entities);
        #[cfg(test)]
        self.pause_before_entity_goal_compute_for_test();
        #[cfg(feature = "load-bench")]
        let terrain_started = std::time::Instant::now();
        let goal_budget = PathingBudget {
            max_candidates_per_entity: pathing_candidates_per_entity.max(1),
            ..PathingBudget::DEFAULT
        };
        let pathing_request_count = prepared_goal_tick.pathing_request_count();
        let pathing_aabbs = prepared_goal_tick.pathing_aabbs().clone();
        let terrain_snapshot = if terrain_pathing_entities.is_empty() && pathing_aabbs.is_empty() {
            None
        } else {
            world_read.zip(pathing_materials).map(|(world_read, _)| {
                let chunks = if pathing_request_count >= active_chunks.len() {
                    let chunks = active_chunks
                        .iter()
                        .map(|&(x, z)| mc_world::ChunkPos { x, z })
                        .collect::<HashSet<_>>();
                    sorted_chunk_positions(chunks)
                } else {
                    let mut chunks = HashSet::new();
                    prepared_goal_tick.visit_pathing_probe_positions(
                        goal_budget,
                        |entity, position| {
                            insert_terrain_snapshot_chunks_for_probe_position(
                                &mut chunks,
                                entity,
                                position,
                                &terrain_pathing_entities,
                                &pathing_aabbs,
                                &active_chunks,
                            );
                        },
                    );
                    sorted_chunk_positions(chunks)
                };
                world_read.snapshot_chunks(&chunks)
            })
        };
        #[cfg(feature = "load-bench")]
        let terrain_us = u64::try_from(terrain_started.elapsed().as_micros()).unwrap_or(u64::MAX);
        #[cfg(feature = "load-bench")]
        let resolve_started = std::time::Instant::now();
        let pathing_probe = LoadedChunkPathingProbe::new(
            &active_chunks,
            &terrain_pathing_entities,
            &pathing_aabbs,
            terrain_snapshot
                .as_ref()
                .zip(pathing_materials)
                .map(|(snapshot, materials)| LoadedTerrainPathingProbe::new(snapshot, materials)),
        );
        let goal_cpu_permits = cpu_resources
            .map(|resources| {
                acquire_regional_worker_permits(
                    resources,
                    prepared_goal_tick.parallel_batch_count(),
                )
            })
            .unwrap_or_default();
        let worker_count = goal_cpu_permits.len() + 1;
        let resolved_goal_tick = if worker_count > 1 {
            prepared_goal_tick.resolve_parallel(&pathing_probe, goal_budget, worker_count)
        } else {
            prepared_goal_tick.resolve(&pathing_probe, goal_budget)
        };
        let resolved_direct_paths = pathing_probe
            .resolved_direct_paths
            .into_inner()
            .expect("entity pathing result lock poisoned");
        #[cfg(feature = "load-bench")]
        let resolve_us = u64::try_from(resolve_started.elapsed().as_micros()).unwrap_or(u64::MAX);
        #[cfg(feature = "load-bench")]
        let apply_started = std::time::Instant::now();
        let mut entities = self.lock_entities("apply entity goals");
        if self.live_session_generation.load(Ordering::Acquire) != live_session_generation {
            drop(goal_cpu_permits);
            drop(entities);
            self.clear_active_simulation_selection();
            return (
                Vec::new(),
                None,
                VillagerPopulationSelection::default(),
                None,
            );
        }
        let (results, goal_applied, owner_fence) = match entities
            .apply_prepared_goal_tick_and_simulation_results(
                resolved_goal_tick,
                Arc::clone(&planned_goal_ids),
            ) {
            Some((_, mut results, owner_fence)) => {
                if !deferred_goal_ids.is_empty() {
                    let (continued, _) =
                        entities.current_simulation_results_for_ids(&deferred_goal_ids);
                    results.extend(continued);
                }
                (results, true, owner_fence)
            }
            None => {
                let (results, owner_fence) =
                    entities.current_simulation_results_for_ids(&goal_population_ids);
                (results, false, owner_fence)
            }
        };
        if goal_applied {
            let _cross_region_gossip_transfers = apply_cross_region_villager_gossip_transfers(
                &mut entities,
                &active_villager_ids,
                &cross_region_villager_candidates,
                tick,
            );
        }
        let villager_metadata_updates = villager_profession_metadata
            .iter()
            .filter_map(|(entity, profession)| {
                let snapshot = entities.snapshot(*entity)?;
                snapshot
                    .retained
                    .villager
                    .is_some_and(|villager| villager.profession == *profession)
                    .then_some(snapshot)
            })
            .collect::<Vec<_>>();
        let cleared_overrides = overridden_villagers
            .iter()
            .copied()
            .filter(|entity| {
                entities.snapshot(*entity).is_none_or(|snapshot| {
                    snapshot.lifecycle != EntityLifecycle::Alive
                        || current_villager_brain(&snapshot)
                            .is_none_or(|brain| brain.override_order.is_none())
                })
            })
            .collect::<Vec<_>>();
        self.clear_villager_overrides(&cleared_overrides);
        #[cfg(feature = "load-bench")]
        let apply_us = u64::try_from(apply_started.elapsed().as_micros()).unwrap_or(u64::MAX);
        #[cfg(feature = "load-bench")]
        let post_started = std::time::Instant::now();
        drop(goal_cpu_permits);
        drop(entities);
        let finished = self.finish_entity_tick_selection(
            live_session_generation,
            simulation_chunks,
            active_population_count,
            results,
            owner_fence,
            goal_applied,
            resolved_direct_paths,
            villager_metadata_updates,
            regional_tick.take().map(|(_, _, output)| output),
        );
        #[cfg(feature = "load-bench")]
        {
            let post_us = u64::try_from(post_started.elapsed().as_micros()).unwrap_or(u64::MAX);
            if tick.is_multiple_of(10) {
                tracing::trace!(
                    "ENTITY_GOAL_PHASE tick={tick} active={} selected={} goal_ids={} selection_us={selection_us} projection_us={projection_us} targets_us={targets_us} prepare_us={prepare_us} terrain_us={terrain_us} resolve_us={resolve_us} apply_us={apply_us} post_us={post_us}",
                    self.entity_update_active_population.load(Ordering::Relaxed),
                    self.entity_update_selected.load(Ordering::Relaxed),
                    goal_entity_count,
                );
            }
        }
        finished
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_entity_tick_selection(
        &self,
        live_session_generation: u64,
        simulation_chunks: HashSet<(i32, i32)>,
        _active_population_count: usize,
        results: Vec<EntitySimulationResult>,
        owner_fence: Option<mc_entity::VersionedEntityKinematics>,
        mut goal_applied: bool,
        mut resolved_direct_paths: HashSet<EntityId>,
        mut villager_metadata_updates: Vec<EntitySnapshot>,
        regional_output: Option<mc_entity::RegionalEntityTickOutput>,
    ) -> (
        Vec<EntityPhysicsQuery>,
        Option<mc_entity::VersionedEntityKinematics>,
        VillagerPopulationSelection,
        Option<mc_entity::RegionalPreparedEntityPhysics>,
    ) {
        let (
            regional_active_hostiles,
            regional_villager_candidates,
            regional_villagers,
            regional_proximity_seeds,
            regional_prepared,
        ) = if let Some(output) = regional_output {
            goal_applied |= output.active_entity_count > output.fallback_entity_ids.len();
            resolved_direct_paths.extend(output.resolved_direct_paths);
            villager_metadata_updates.extend(output.villager_profession_updates);
            (
                output.active_hostile_ids,
                output.villager_population_candidates,
                output.villager_ids,
                output.villager_proximity_seeds,
                Some(output.prepared_physics),
            )
        } else {
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), None)
        };
        let mut active_hostile_ids = HashSet::new();
        let mut villager_population = VillagerPopulationSelection::default();
        let mut queries = Vec::with_capacity(results.len());
        {
            let mut classify = |result: EntitySimulationResult, include_query: bool| {
                let position = result.physics.position;
                if !simulation_chunks.contains(&chunk_pos_from_coords(position.x, position.z)) {
                    return;
                }
                if result.hostile {
                    active_hostile_ids.insert(result.physics.id);
                }
                if result.villager_population_active {
                    villager_population.candidates.insert(result.physics.id);
                }
                if result.villager {
                    villager_population.covered.insert(result.physics.id);
                }
                if result.villager_population_active || result.item {
                    villager_population.proximity_seeds.push(position);
                }
                if include_query {
                    queries.push(result.physics);
                }
            };
            for result in results {
                classify(result, true);
            }
            active_hostile_ids.extend(regional_active_hostiles.into_iter().filter_map(
                |(entity, position)| {
                    simulation_chunks
                        .contains(&chunk_pos_from_coords(position.x, position.z))
                        .then_some(entity)
                },
            ));
            villager_population.candidates.extend(
                regional_villager_candidates
                    .into_iter()
                    .filter_map(|(entity, position)| {
                        simulation_chunks
                            .contains(&chunk_pos_from_coords(position.x, position.z))
                            .then_some(entity)
                    }),
            );
            villager_population
                .covered
                .extend(
                    regional_villagers
                        .into_iter()
                        .filter_map(|(entity, position)| {
                            simulation_chunks
                                .contains(&chunk_pos_from_coords(position.x, position.z))
                                .then_some(entity)
                        }),
                );
            villager_population.proximity_seeds.extend(
                regional_proximity_seeds
                    .into_iter()
                    .filter_map(|(_, position)| {
                        simulation_chunks
                            .contains(&chunk_pos_from_coords(position.x, position.z))
                            .then_some(position)
                    }),
            );
        }
        #[cfg(test)]
        self.active_entity_selection_visits.fetch_add(
            u64::try_from(_active_population_count).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.publish_active_simulation_selection(
            live_session_generation,
            simulation_chunks,
            active_hostile_ids,
        );
        self.publish_villager_metadata_updates(villager_metadata_updates);
        if goal_applied && !resolved_direct_paths.is_empty() {
            self.simulation_inputs
                .remove_terrain_pathing(resolved_direct_paths);
        }
        (queries, owner_fence, villager_population, regional_prepared)
    }

    pub(crate) fn commit_owned_region_physics(
        &self,
        prepared: mc_entity::RegionalPreparedEntityPhysics,
    ) -> (
        Vec<EntityPhysicsQuery>,
        Option<mc_entity::VersionedEntityKinematics>,
        Option<RegionallyCommittedEntityMovement>,
        mc_entity::LaneCommitTimings,
    ) {
        let output = self.entities.commit_owned_region_physics(prepared);
        self.simulation_inputs
            .insert_terrain_pathing(output.terrain_pathing_additions);
        let queries = output
            .fallback_results
            .into_iter()
            .map(|result| result.physics)
            .collect();
        let commit =
            (!output.committed_motion.is_empty()).then_some(RegionallyCommittedEntityMovement {
                states: output.committed_motion,
                fence: output.publication_fence,
            });
        (queries, output.fallback_fence, commit, output.lane_timings)
    }

    pub(in crate::play) fn restore_persisted_entities_owned(
        &self,
        _authority: &SimulationAuthority,
        checkpoint: PersistedEntityCheckpoint,
    ) -> usize {
        self.restore_persisted_entities_core(checkpoint)
    }

    #[cfg(test)]
    pub(crate) fn restore_persisted_entities(
        &self,
        checkpoint: PersistedEntityCheckpoint,
    ) -> usize {
        self.restore_persisted_entities_core(checkpoint)
    }

    fn restore_persisted_entities_core(&self, checkpoint: PersistedEntityCheckpoint) -> usize {
        let mut inner = self.lock_session_entities("restore persisted entities");
        let current_clock = self.simulation_tick();
        let owner_is_empty = inner.entities.snapshots_vec().is_empty();
        if !checkpoint.has_valid_temporal_state()
            || (current_clock != checkpoint.lifecycle_clock
                && !(owner_is_empty && current_clock == 0))
        {
            return 0;
        }
        let PersistedEntityCheckpoint {
            lifecycle_clock,
            regional_sequence_watermark,
            records,
            settlement_claims,
        } = checkpoint;
        self.entities
            .restore_checkpoint_boundary(lifecycle_clock, regional_sequence_watermark);
        if !inner
            .entities
            .insert_snapshots_batch(records.iter().map(|record| record.snapshot.clone()))
        {
            return 0;
        }
        inner.entity_lifecycle_tick = lifecycle_clock;
        inner.settlement_spawn_claims = settlement_claims;
        let restored_snapshots = records
            .iter()
            .map(|record| record.snapshot.clone())
            .collect::<Vec<_>>();
        super::villager_population::rebuild_villager_population_indexes_locked(
            &mut inner,
            &restored_snapshots,
        );
        if current_clock != lifecycle_clock {
            self.entity_lifecycle_tick
                .store(lifecycle_clock, Ordering::Release);
            self.simulation_tick_sender.send_replace(lifecycle_clock);
        }
        let restored = records.len();
        for record in records {
            let entity = record.snapshot;
            let aabb = entity_aabb(&entity.type_name);
            let type_id = entity.type_id;
            let entity_id = entity.id;
            let position = entity.position;
            let natural_mob = if is_hostile_entity(&entity.type_name) {
                inner.hostile_entities.insert(entity_id);
                inner.natural_hostile_mobs.insert(entity_id);
                true
            } else if entity_type_uses_aquatic_physics(&entity.type_name) {
                inner.natural_aquatic_mobs.insert(entity_id);
                true
            } else if entity.animal.is_some() {
                inner.natural_ground_mobs.insert(entity_id);
                true
            } else {
                false
            };
            if natural_mob {
                inner
                    .natural_mob_no_action_since_tick
                    .insert(entity_id, lifecycle_clock);
            }
            if entity.type_name == "minecraft:sheep" {
                inner.sheep_entities.insert(entity_id);
            }
            update_breeding_tick_tracking_locked(&mut inner, entity_id, entity.animal);
            schedule_entity_death_locked(&mut inner, &entity);
            super::zombie_villager::schedule_zombie_villager_conversion_locked(&mut inner, &entity);
            schedule_primed_tnt_deadline_locked(
                &mut inner,
                entity_id,
                entity.retained.primed_tnt.map(|fuse| fuse.expires_tick),
            );
            if entity.item_stack.is_some() {
                schedule_item_despawn_locked(&mut inner, entity_id, entity.retained.spawn_tick);
            }
            if entity.item_stack.is_some()
                && let Some(ready_tick) = entity.retained.item_pickup_ready_tick
                && ready_tick > lifecycle_clock
            {
                inner
                    .item_pickup_ready
                    .entry(ready_tick)
                    .or_default()
                    .push(entity_id);
            }
            inner.entity_type_aabbs.entry(type_id).or_insert(aabb);
            track_entity_chunk_locked(&mut inner, entity_id, position);
            initialize_entity_wire_state_locked(&mut inner, entity_id);
            let _ = publish_server_entity_snapshot_locked(&mut inner, entity_id);
        }
        restored
    }

    #[cfg(test)]
    pub(crate) fn persisted_entity_records(&self) -> Vec<PersistedEntityRecord> {
        self.persisted_entity_save_snapshot().0.records
    }

    pub(crate) fn persisted_entity_save_snapshot(
        &self,
    ) -> (PersistedEntityCheckpoint, Vec<mc_entity::RegionPhase>) {
        let metadata = EntityPersistenceMetadata {
            lifecycle_tick: self.simulation_tick(),
        };
        self.entities
            .advance_lifecycle_epoch(metadata.lifecycle_tick);
        #[cfg(test)]
        self.pause_before_entity_save_owner_barrier_for_test();
        let saved = owner_result(&self.entities, self.entities.handle.save_barrier());
        let (mut checkpoint, phases) = project_owner_save(saved, &metadata);
        checkpoint.settlement_claims = self
            .lock_inner("snapshot settlement spawn claims")
            .settlement_spawn_claims
            .clone();
        (checkpoint, phases)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn apply_entity_physics_if_current_and_dispatch_regional(
        &self,
        cpu_resources: &crate::chunk_pipeline::ChunkPipelineResources,
        tick: u64,
        expected: &[EntityPhysicsQuery],
        steps: &[EntityPhysicsStep],
        owner_fence: Option<mc_entity::VersionedEntityKinematics>,
        projectile_physics_facts: &EntityProjectilePhysicsFacts,
        mut regional_commit: Option<RegionallyCommittedEntityMovement>,
    ) -> Vec<EntityPhysicsStep> {
        let no_due_item_despawn = {
            let inner = self.lock_inner("check entity item despawn deadline");
            inner
                .item_despawn_deadlines
                .first_key_value()
                .is_none_or(|(&deadline, _)| deadline > tick)
        };
        let publish_before_central = no_due_item_despawn
            && projectile_physics_facts.arrows.is_empty()
            && projectile_physics_facts.hurting.is_empty()
            && projectile_physics_facts.throwable.is_empty();
        if publish_before_central && let Some(commit) = regional_commit.take() {
            self.entity_lifecycle_tick.fetch_max(tick, Ordering::AcqRel);
            self.publish_regionally_committed_entity_movement(tick, commit);
        }
        let accepted = self.apply_entity_physics_and_dispatch_core(
            Some(cpu_resources),
            tick,
            Some(expected),
            steps,
            owner_fence,
            projectile_physics_facts,
        );
        if let Some(commit) = regional_commit {
            self.publish_regionally_committed_entity_movement(tick, commit);
        }
        accepted
    }

    #[cfg(test)]
    pub(crate) fn apply_entity_physics_and_dispatch(&self, tick: u64, steps: &[EntityPhysicsStep]) {
        let projectile_physics_facts = test_entity_projectile_physics_facts(steps);
        let _ = self.apply_entity_physics_and_dispatch_core(
            None,
            tick,
            None,
            steps,
            None,
            &projectile_physics_facts,
        );
    }

    #[cfg(test)]
    pub(crate) fn apply_entity_physics_with_arrow_facts_and_dispatch(
        &self,
        tick: u64,
        steps: &[EntityPhysicsStep],
        arrow_physics_facts: &[ArrowPhysicsFact],
    ) {
        let projectile_physics_facts = EntityProjectilePhysicsFacts {
            arrows: arrow_physics_facts.to_vec(),
            hurting: Vec::new(),
            throwable: Vec::new(),
        };
        let _ = self.apply_entity_physics_and_dispatch_core(
            None,
            tick,
            None,
            steps,
            None,
            &projectile_physics_facts,
        );
    }

    #[cfg(test)]
    pub(crate) fn apply_entity_physics_with_hurting_facts_and_dispatch(
        &self,
        tick: u64,
        steps: &[EntityPhysicsStep],
        hurting_physics_facts: &[HurtingProjectilePhysicsFact],
    ) {
        let projectile_physics_facts = EntityProjectilePhysicsFacts {
            arrows: Vec::new(),
            hurting: hurting_physics_facts.to_vec(),
            throwable: Vec::new(),
        };
        let _ = self.apply_entity_physics_and_dispatch_core(
            None,
            tick,
            None,
            steps,
            None,
            &projectile_physics_facts,
        );
    }

    #[cfg(test)]
    pub(crate) fn apply_entity_physics_with_throwable_facts_and_dispatch(
        &self,
        tick: u64,
        steps: &[EntityPhysicsStep],
        throwable_physics_facts: &[HurtingProjectilePhysicsFact],
    ) {
        let projectile_physics_facts = EntityProjectilePhysicsFacts {
            arrows: Vec::new(),
            hurting: Vec::new(),
            throwable: throwable_physics_facts.to_vec(),
        };
        let _ = self.apply_entity_physics_and_dispatch_core(
            None,
            tick,
            None,
            steps,
            None,
            &projectile_physics_facts,
        );
    }

    #[cfg(test)]
    pub(crate) fn apply_entity_physics_if_current_and_dispatch(
        &self,
        tick: u64,
        expected: &[EntityPhysicsQuery],
        steps: &[EntityPhysicsStep],
    ) {
        let projectile_physics_facts = test_entity_projectile_physics_facts(steps);
        let _ = self.apply_entity_physics_and_dispatch_core(
            None,
            tick,
            Some(expected),
            steps,
            None,
            &projectile_physics_facts,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn apply_entity_physics_and_dispatch_core(
        &self,
        cpu_resources: Option<&crate::chunk_pipeline::ChunkPipelineResources>,
        tick: u64,
        expected: Option<&[EntityPhysicsQuery]>,
        steps: &[EntityPhysicsStep],
        owner_fence: Option<mc_entity::VersionedEntityKinematics>,
        projectile_physics_facts: &EntityProjectilePhysicsFacts,
    ) -> Vec<EntityPhysicsStep> {
        self.entity_lifecycle_tick.fetch_max(tick, Ordering::AcqRel);
        #[cfg(feature = "load-bench")]
        let mut profile = PhysicsApplyProfile::new(tick, steps.len());
        #[cfg(feature = "load-bench")]
        let preflight_lock_started = std::time::Instant::now();
        let mut entities = self.lock_entities("prepare entity physics");
        #[cfg(feature = "load-bench")]
        {
            profile.preflight_lock_us = PhysicsApplyProfile::elapsed_us(preflight_lock_started);
        }
        #[cfg(feature = "load-bench")]
        let preflight_prefetch_started = std::time::Instant::now();
        let mut owner_fence = owner_fence;
        let direct_queries = expected.filter(|queries| {
            queries.len() == steps.len()
                && queries
                    .iter()
                    .zip(steps)
                    .all(|(query, step)| query.id == step.id)
        });
        let direct_fence = direct_queries.and_then(|_| owner_fence.take());
        let direct_attempted = direct_fence.is_some();
        let publish_ids = if direct_attempted {
            let mut publish_ids = {
                let inner = self.lock_inner("select smooth natural entity movement");
                inner
                    .natural_hostile_mobs
                    .iter()
                    .chain(&inner.natural_ground_mobs)
                    .chain(&inner.natural_aquatic_mobs)
                    .copied()
                    .collect::<HashSet<_>>()
            };
            if let Some(queries) = direct_queries {
                publish_ids.extend(queries.iter().zip(steps).filter_map(|(query, step)| {
                    (chunk_pos_from_coords(query.position.x, query.position.z)
                        != chunk_pos_from_coords(step.position.x, step.position.z))
                    .then_some(step.id)
                }));
            }
            if tick.is_multiple_of(ENTITY_MOVE_SEND_INTERVAL_TICKS) {
                let ordinary_count = steps.len();
                let budget = self.entity_movement_publication_budget();
                publish_ids.extend(steps.iter().enumerate().filter_map(|(ordinal, step)| {
                    ordinary_entity_is_due_for_movement_tracking(
                        ordinal,
                        tick,
                        ordinary_count,
                        budget,
                    )
                    .then_some(step.id)
                }));
            }
            publish_ids
        } else {
            HashSet::new()
        };
        let direct_result = if let (Some(fence), Some(queries)) = (direct_fence, direct_queries) {
            #[cfg(test)]
            self.pause_before_physics_owner_apply_for_test();
            entities.apply_entity_physics_from_fence(fence, queries, steps, publish_ids)
        } else {
            None
        };
        let (direct_routed, direct_rejected, direct_motion, mut applied_kinematics) =
            match direct_result {
                Some((rejected, motion, kinematics)) => (true, rejected, motion, kinematics),
                None => (false, Vec::new(), Vec::new(), Vec::new()),
            };
        let direct_complete = direct_routed && direct_rejected.is_empty();
        let direct_rejected = direct_rejected.into_iter().collect::<HashSet<_>>();
        let mut old_motion = direct_motion
            .into_iter()
            .map(|motion| (motion.id, motion))
            .collect::<HashMap<_, _>>();
        let fallback_ids = if direct_complete {
            HashSet::new()
        } else if direct_routed {
            direct_rejected.clone()
        } else {
            steps.iter().map(|step| step.id).collect::<HashSet<_>>()
        };
        let fallback_preferred = (!direct_attempted).then_some(owner_fence).flatten();
        let (kinematics_fence, prefetch_missing) =
            entities.prefetch_kinematics(&fallback_ids, fallback_preferred);
        #[cfg(feature = "load-bench")]
        {
            profile.prefetch_missing = prefetch_missing;
            profile.preflight_prefetch_us =
                PhysicsApplyProfile::elapsed_us(preflight_prefetch_started);
        }
        #[cfg(not(feature = "load-bench"))]
        let _ = prefetch_missing;
        #[cfg(feature = "load-bench")]
        let preflight_capture_started = std::time::Instant::now();
        let expected_by_id = (!fallback_ids.is_empty())
            .then(|| {
                expected.map(|queries| {
                    queries
                        .iter()
                        .filter(|query| fallback_ids.contains(&query.id))
                        .map(|query| (query.id, *query))
                        .collect::<HashMap<_, _>>()
                })
            })
            .flatten();
        let mut handled_steps = Vec::with_capacity(steps.len());
        let mut publication_steps = Vec::new();
        let mut old_chunks: HashMap<_, _> = HashMap::new();
        let mut kinematics = Vec::with_capacity(fallback_ids.len());
        let mut selected_motion_cursor = None;
        for step in steps {
            if !step.position.is_finite() || !step.velocity.is_finite() {
                continue;
            }
            if direct_complete || (direct_routed && !direct_rejected.contains(&step.id)) {
                handled_steps.push(*step);
                if old_motion.contains_key(&step.id) {
                    publication_steps.push(*step);
                    if let Some(chunk) = self.simulation_inputs.entity_chunk(step.id) {
                        old_chunks.insert(step.id, chunk);
                    }
                }
                continue;
            }
            let expected = expected_by_id
                .as_ref()
                .and_then(|by_id| by_id.get(&step.id));
            if expected_by_id.is_some() && expected.is_none() {
                continue;
            }
            #[cfg(feature = "load-bench")]
            let sample_motion_lookup = profile.motion_lookup_calls.is_multiple_of(1024);
            #[cfg(feature = "load-bench")]
            let motion_lookup_started = sample_motion_lookup.then(std::time::Instant::now);
            let current_motion = entities.motion_state_with_fence_cursor(
                kinematics_fence.as_ref(),
                step.id,
                &mut selected_motion_cursor,
            );
            #[cfg(feature = "load-bench")]
            {
                profile.motion_lookup_calls = profile.motion_lookup_calls.saturating_add(1);
                if let Some(started) = motion_lookup_started {
                    profile.motion_lookup_samples = profile.motion_lookup_samples.saturating_add(1);
                    profile.motion_lookup_sample_us = profile
                        .motion_lookup_sample_us
                        .saturating_add(PhysicsApplyProfile::elapsed_us(started));
                }
            }
            let Some(current_motion) = current_motion else {
                continue;
            };
            if let Some(expected) = expected
                && !expected.matches_motion(current_motion)
            {
                continue;
            }
            handled_steps.push(*step);
            publication_steps.push(*step);
            if let Some(chunk) = self.simulation_inputs.entity_chunk(step.id) {
                old_chunks.insert(step.id, chunk);
            }
            old_motion.insert(step.id, current_motion);
            if !current_motion.is_arrow
                && !current_motion.is_hurting_projectile
                && !current_motion.is_throwable_projectile
            {
                kinematics.push(EntityKinematics {
                    id: step.id,
                    position: step.position,
                    rotation: current_motion.rotation,
                    velocity: step.velocity,
                    on_ground: step.on_ground,
                });
            }
        }
        #[cfg(feature = "load-bench")]
        {
            profile.preflight_capture_us =
                PhysicsApplyProfile::elapsed_us(preflight_capture_started);
        }
        #[cfg(feature = "load-bench")]
        let preflight_finalize_started = std::time::Instant::now();
        let publication_ids = publication_steps
            .iter()
            .map(|step| step.id)
            .collect::<HashSet<_>>();
        #[cfg(feature = "load-bench")]
        profile.record_filtered(publication_steps.len());
        let regional_batch_count = entities.parallel_kinematics_batch_count(&kinematics);
        let regional_worker_permits = cpu_resources
            .map(|resources| acquire_regional_worker_permits(resources, regional_batch_count))
            .unwrap_or_default();
        #[cfg(feature = "load-bench")]
        {
            profile.preflight_finalize_us =
                PhysicsApplyProfile::elapsed_us(preflight_finalize_started);
        }
        #[cfg(feature = "load-bench")]
        profile.finish_preflight();
        #[cfg(test)]
        if !direct_attempted {
            self.pause_before_physics_owner_apply_for_test();
        }
        let fallback_applied = if regional_worker_permits.is_empty() {
            entities.apply_kinematics_authoritative_with_fence(kinematics, kinematics_fence)
        } else {
            entities.apply_kinematics_parallel_authoritative_with_fence(
                kinematics,
                kinematics_fence,
                regional_worker_permits.len() + 1,
            )
        };
        applied_kinematics.extend(fallback_applied);
        applied_kinematics.sort_unstable_by_key(|state| state.id);
        drop(regional_worker_permits);
        #[cfg(feature = "load-bench")]
        profile.finish_owner_apply(applied_kinematics.len(), entities.take_fenced_apply());
        // Re-read the owner state before taking the publication lock:
        // regional owner mutation does not require this lock, so snapshots
        // fetched before it is acquired are only a valid publication view
        // while their versioned fence stays current. The fence is validated
        // cheaply under the lock; a stale fence drops the lock and refetches
        // outside it instead of reading the owner under the session registry
        // mutex.
        #[cfg(feature = "load-bench")]
        profile.begin_publication_refresh();
        let mut publication_snapshots =
            entities.take_current_publication_snapshots(&publication_ids);
        if publication_snapshots.is_none() {
            publication_snapshots = entities.refresh_publication_snapshots(&publication_ids);
        }
        #[cfg(feature = "load-bench")]
        profile.finish_publication_refresh();
        #[cfg(feature = "load-bench")]
        profile.begin_locked_publish();
        let session_inner = loop {
            let session_inner = self.lock_inner("publish entity physics");
            let publication_current = publication_snapshots
                .as_ref()
                .is_none_or(|fence| entities.versioned_snapshots_are_current(fence));
            if publication_current {
                break session_inner;
            }
            drop(session_inner);
            #[cfg(feature = "load-bench")]
            profile.record_publication_retry();
            #[cfg(feature = "load-bench")]
            profile.begin_publication_refresh();
            publication_snapshots = entities.refresh_publication_snapshots(&publication_ids);
            #[cfg(feature = "load-bench")]
            profile.finish_publication_refresh();
        };
        let steps = publication_steps.as_slice();
        let mut inner = SessionEntityGuards {
            inner: session_inner,
            entities,
            entity_lifecycle_tick: self.simulation_tick(),
        };
        let mut dispatches = despawn_expired_items_locked(&mut inner);
        let input_steps = steps
            .iter()
            .map(|step| (step.id, *step))
            .collect::<HashMap<_, _>>();
        let applied_motion = applied_kinematics
            .into_iter()
            .filter_map(|state| {
                let mut current = *old_motion.get(&state.id)?;
                current.position = state.position;
                current.rotation = state.rotation;
                current.velocity = state.velocity;
                current.on_ground = state.on_ground;
                Some((state, current))
            })
            .collect::<Vec<_>>();
        let applied_motion_by_id = applied_motion
            .iter()
            .map(|(_, motion)| (motion.id, *motion))
            .collect::<HashMap<_, _>>();
        let mut applied_steps = applied_motion
            .iter()
            .filter_map(|(state, _)| {
                let input = input_steps.get(&state.id)?;
                Some(EntityPhysicsStep {
                    id: state.id,
                    position: state.position,
                    velocity: state.velocity,
                    on_ground: state.on_ground,
                    horizontal_collision: input.horizontal_collision,
                })
            })
            .collect::<Vec<_>>();
        for (_, motion) in &applied_motion {
            publish_server_entity_motion_locked(&mut inner, *motion);
        }
        inner = resolve_arrow_entity_hits_locked(
            self,
            inner,
            steps,
            &old_motion,
            &projectile_physics_facts.arrows,
            &mut dispatches,
        );
        let dragon_cloud_entity_type_id = self
            .hostile_area_effect_cloud_entity_type_id
            .load(Ordering::Acquire);
        let dragon_cloud_entity_type_id =
            (dragon_cloud_entity_type_id >= 0).then_some(dragon_cloud_entity_type_id);
        let (resolved_inner, hurting_steps) = resolve_hurting_projectile_hits_locked(
            inner,
            steps,
            &old_motion,
            &projectile_physics_facts.hurting,
            dragon_cloud_entity_type_id,
            &mut dispatches,
        );
        inner = resolved_inner;
        applied_steps.extend(hurting_steps);
        let (resolved_inner, throwable_steps) = resolve_throwable_projectile_hits_locked(
            inner,
            steps,
            &old_motion,
            &projectile_physics_facts.throwable,
            &mut dispatches,
        );
        inner = resolved_inner;
        applied_steps.extend(throwable_steps);
        let mut rejected_arrows = std::mem::take(&mut inner.arrow_tick_scratch.rejected);
        let mut processed_arrows = std::mem::take(&mut inner.arrow_tick_scratch.processed);
        applied_steps.extend(steps.iter().copied().filter(|step| {
            processed_arrows.contains(&step.id)
                && old_motion
                    .get(&step.id)
                    .is_some_and(|motion| motion.is_arrow)
        }));
        #[cfg(feature = "load-bench")]
        profile.record_applied_steps(applied_steps.len());
        let effective_steps = applied_steps
            .iter()
            .filter(|step| !rejected_arrows.contains(&step.id))
            .filter_map(|step| {
                let motion = applied_motion_by_id
                    .get(&step.id)
                    .copied()
                    .or_else(|| inner.entities.motion_state(step.id))?;
                Some(EntityPhysicsStep {
                    id: step.id,
                    position: motion.position,
                    velocity: motion.velocity,
                    on_ground: motion.on_ground,
                    horizontal_collision: step.horizontal_collision,
                })
            })
            .collect::<Vec<_>>();
        rejected_arrows.clear();
        inner.arrow_tick_scratch.rejected = rejected_arrows;
        processed_arrows.clear();
        inner.arrow_tick_scratch.processed = processed_arrows;
        #[cfg(feature = "load-bench")]
        profile.record_effective(effective_steps.len());
        let effective_by_id = effective_steps
            .iter()
            .map(|step| (step.id, *step))
            .collect::<HashMap<_, _>>();
        handled_steps.retain_mut(|step| {
            if !publication_ids.contains(&step.id) {
                return true;
            }
            let Some(effective) = effective_by_id.get(&step.id).copied() else {
                return false;
            };
            *step = effective;
            true
        });
        let steps = effective_steps.as_slice();
        let terrain_pathing_additions = steps
            .iter()
            .filter(|step| step.horizontal_collision && step.velocity.y <= 0.0)
            .map(|step| step.id)
            .collect::<Vec<_>>();
        inner
            .simulation_inputs
            .insert_terrain_pathing(terrain_pathing_additions);
        let mut chunk_crossings = steps
            .iter()
            .filter_map(|step| {
                let old_chunk = old_chunks.get(&step.id).copied()?;
                let new_chunk = chunk_pos_from_coords(step.position.x, step.position.z);
                (old_chunk != new_chunk).then_some((step.id, old_chunk, new_chunk))
            })
            .collect::<Vec<_>>();
        let routing_moves = chunk_crossings
            .iter()
            .map(
                |&(entity, expected_chunk, new_chunk)| ExpectedEntityRoutingMove {
                    entity,
                    expected_chunk,
                    new_chunk,
                },
            )
            .collect::<Vec<_>>();
        if !routing_moves.is_empty() {
            let entity_lifecycle_tick = inner.entity_lifecycle_tick;
            let SessionEntityGuards {
                inner: session_inner,
                entities,
                ..
            } = inner;
            drop(session_inner);
            #[cfg(test)]
            self.pause_before_physics_routing_for_test();
            let routing_outcomes = self
                .simulation_inputs
                .move_entities_if_current(&routing_moves);
            debug_assert_eq!(routing_outcomes.len(), chunk_crossings.len());
            chunk_crossings = chunk_crossings
                .into_iter()
                .zip(routing_outcomes)
                .filter_map(|(crossing, outcome)| {
                    debug_assert_eq!(outcome.entity, crossing.0);
                    if !outcome.applied {
                        debug_assert_ne!(outcome.current_chunk, Some(crossing.1));
                    }
                    outcome.applied.then_some(crossing)
                })
                .collect();
            let session_inner = self.lock_inner("publish entity chunk crossings");
            inner = SessionEntityGuards {
                inner: session_inner,
                entities,
                entity_lifecycle_tick,
            };
        }
        #[cfg(feature = "load-bench")]
        profile.record_chunk_crossings(chunk_crossings.len());
        let old_observers_by_entity = chunk_crossings
            .iter()
            .map(|&(entity_id, _, _)| {
                #[cfg(test)]
                self.physics_boundary_observer_scans
                    .fetch_add(1, Ordering::Relaxed);
                (
                    entity_id,
                    visible_entity_observers_locked(&inner, entity_id)
                        .into_iter()
                        .collect::<HashSet<_>>(),
                )
            })
            .collect::<HashMap<_, _>>();
        for &(entity_id, old_chunk, new_chunk) in &chunk_crossings {
            debug_assert_eq!(
                inner.simulation_inputs.entity_chunk(entity_id),
                Some(new_chunk)
            );
            debug_assert_ne!(old_chunk, new_chunk);
        }
        for &(entity_id, old_chunk, new_chunk) in &chunk_crossings {
            let Some(old_observers) = old_observers_by_entity.get(&entity_id) else {
                continue;
            };
            dispatches.extend(refresh_entity_target_visibility_with_old_observers_locked(
                &mut inner,
                entity_id,
                old_chunk,
                new_chunk,
                old_observers,
            ));
        }
        let item_ids = steps
            .iter()
            .filter(|step| {
                old_motion
                    .get(&step.id)
                    .is_some_and(|motion| motion.is_item)
            })
            .map(|step| step.id)
            .collect::<Vec<_>>();
        dispatches.extend(merge_item_entities_locked(&mut inner, &item_ids));
        let ordinary_tracking_turn = tick.is_multiple_of(ENTITY_MOVE_SEND_INTERVAL_TICKS);
        let movement_publication_budget = self.entity_movement_publication_budget();
        let mut tracker_inputs = Vec::with_capacity(steps.len());
        for step in steps {
            let Some(motion) = applied_motion_by_id
                .get(&step.id)
                .copied()
                .or_else(|| inner.entities.motion_state(step.id))
            else {
                continue;
            };
            let latency_sensitive = motion.is_arrow || motion.is_item || motion.is_experience;
            let smooth_natural_mob = inner.natural_hostile_mobs.contains(&step.id)
                || inner.natural_ground_mobs.contains(&step.id)
                || inner.natural_aquatic_mobs.contains(&step.id);
            if !ordinary_tracking_turn && !latency_sensitive && !smooth_natural_mob {
                continue;
            }
            let last_sent = inner.entity_movement_trackers.get_or_insert(
                step.id,
                LastSentEntityState {
                    position: motion.position,
                    velocity: motion.velocity,
                    rotation: motion.rotation,
                    on_ground: motion.on_ground,
                    tracking_update_count: 0,
                    teleport_delay: 0,
                },
            );
            tracker_inputs.push((motion.into(), last_sent, smooth_natural_mob));
        }
        #[cfg(feature = "load-bench")]
        profile.record_tracker_inputs(tracker_inputs.len());
        let lifecycle_tick = inner.entity_lifecycle_tick;
        let pickup_ready_items = steps
            .iter()
            .filter(|step| {
                old_motion
                    .get(&step.id)
                    .is_some_and(|motion| motion.is_item)
                    && inner.entities.snapshot(step.id).is_some_and(|entity| {
                        entity.retained.item_pickup_claim.is_none()
                            && entity
                                .retained
                                .item_pickup_ready_tick
                                .is_none_or(|ready_tick| ready_tick < lifecycle_tick)
                    })
            })
            .map(|step| step.id)
            .collect::<HashSet<_>>();
        let session_positions = inner
            .sessions
            .iter()
            .map(|(&session_id, session)| {
                (
                    session_id,
                    Vec3::new(session.pose.x, session.pose.y, session.pose.z),
                )
            })
            .collect::<Vec<_>>();
        let entity_movement_trackers = Arc::clone(&inner.entity_movement_trackers);
        let SessionEntityGuards {
            inner: session_inner,
            entities,
            ..
        } = inner;
        drop(entities);
        drop(session_inner);
        #[cfg(feature = "load-bench")]
        profile.finish_locked_publish();
        #[cfg(test)]
        self.pause_before_session_movement_plan_for_test();
        let pickup_positions = steps
            .iter()
            .filter_map(|step| {
                let motion = old_motion.get(&step.id)?;
                (motion.is_experience
                    || (motion.is_item && pickup_ready_items.contains(&step.id))
                    || (motion.is_arrow && step.on_ground && step.velocity == Vec3::ZERO))
                    .then_some(step.position)
            })
            .collect::<Vec<_>>();
        let mut pickup_sessions = if pickup_positions.is_empty() {
            Vec::new()
        } else {
            let radius_sq = ENTITY_PICKUP_RADIUS * ENTITY_PICKUP_RADIUS;
            session_positions
                .iter()
                .filter_map(|&(session_id, player)| {
                    pickup_positions
                        .iter()
                        .any(|position| distance_sq(*position, player) <= radius_sq)
                        .then_some(session_id)
                })
                .collect::<Vec<_>>()
        };
        pickup_sessions.extend(spawned_xp_observer_ids(&dispatches));
        self.dispatch_entity_movement_tracking(
            tick,
            handled_steps,
            dispatches,
            tracker_inputs,
            movement_publication_budget,
            entity_movement_trackers,
            pickup_sessions,
            old_observers_by_entity,
            #[cfg(feature = "load-bench")]
            &mut profile,
        )
    }

    fn publish_regionally_committed_entity_movement(
        &self,
        tick: u64,
        commit: RegionallyCommittedEntityMovement,
    ) {
        let RegionallyCommittedEntityMovement { mut states, fence } = commit;
        #[cfg(feature = "load-bench")]
        let mut profile = PhysicsApplyProfile::new(tick, states.len());
        #[cfg(feature = "load-bench")]
        {
            profile.finish_preflight();
            profile.finish_owner_apply(states.len(), false);
            profile.begin_publication_refresh();
        }
        let mut session_inner = self.lock_inner("publish regional entity movement");
        if !self.entities.versioned_snapshots_are_current(&fence) {
            drop(session_inner);
            states.sort_unstable_by_key(|state| state.id);
            let regional_ids = states.iter().map(|state| state.id).collect::<HashSet<_>>();
            let (mut current_states, current_fence) = self
                .entities
                .alive_kinematics_for_ids_versioned(&regional_ids);
            current_states.sort_unstable_by_key(|state| state.id);
            if current_states.len() != states.len()
                || current_states.iter().zip(&states).any(|(current, motion)| {
                    current.id != motion.id
                        || current.position != motion.position
                        || current.rotation != motion.rotation
                        || current.velocity != motion.velocity
                        || current.on_ground != motion.on_ground
                })
            {
                return;
            }
            let Some(current_fence) = current_fence else {
                return;
            };
            session_inner = self.lock_inner("publish refreshed regional entity movement");
            if !self
                .entities
                .versioned_kinematics_are_current(&current_fence)
            {
                return;
            }
        }
        #[cfg(feature = "load-bench")]
        {
            profile.finish_publication_refresh();
            profile.begin_locked_publish();
        }
        let state_count = states.len();
        let ordinary_tracking_turn = tick.is_multiple_of(ENTITY_MOVE_SEND_INTERVAL_TICKS);
        let movement_publication_budget = self.entity_movement_publication_budget();
        let exceptional_tracking = states
            .iter()
            .any(|motion| motion.is_arrow || motion.is_item || motion.is_experience);
        let priority_tracking = if !exceptional_tracking
            && session_inner.natural_hostile_mobs.is_empty()
            && session_inner.natural_ground_mobs.is_empty()
            && session_inner.natural_aquatic_mobs.is_empty()
        {
            None
        } else {
            Some(
                states
                    .iter()
                    .map(|motion| {
                        motion.is_arrow
                            || motion.is_item
                            || motion.is_experience
                            || session_inner.natural_hostile_mobs.contains(&motion.id)
                            || session_inner.natural_ground_mobs.contains(&motion.id)
                            || session_inner.natural_aquatic_mobs.contains(&motion.id)
                    })
                    .collect::<Vec<_>>(),
            )
        };
        let priority_count = priority_tracking.as_ref().map_or(0, |priorities| {
            priorities.iter().filter(|&&priority| priority).count()
        });
        let ordinary_count = state_count.saturating_sub(priority_count);
        let mut tracker_candidates = Vec::with_capacity(
            priority_count
                + usize::from(ordinary_tracking_turn)
                    * ordinary_count.min(movement_publication_budget),
        );
        let mut ordinary_ordinal = 0usize;
        for (index, motion) in states.into_iter().enumerate() {
            let priority = priority_tracking
                .as_ref()
                .is_some_and(|priorities| priorities[index]);
            debug_assert!(
                !motion.is_arrow && !motion.is_item && !motion.is_experience,
                "exceptional entity movement must stay on the central fallback path"
            );
            if let Some(snapshot) = session_inner.published_entity_snapshots.get_mut(&motion.id) {
                snapshot.position = motion.position;
                snapshot.rotation = motion.rotation;
                snapshot.velocity = motion.velocity;
                snapshot.on_ground = motion.on_ground;
            }
            let selected = priority
                || (ordinary_tracking_turn
                    && ordinary_entity_is_due_for_movement_tracking(
                        ordinary_ordinal,
                        tick,
                        ordinary_count,
                        movement_publication_budget,
                    ));
            ordinary_ordinal += usize::from(!priority);
            if selected {
                // Ordinary candidates are already budgeted here; the shared
                // planner must not rotate this compact selection a second time.
                tracker_candidates.push((motion, true));
            }
        }
        let entity_movement_trackers = Arc::clone(&session_inner.entity_movement_trackers);
        let last_sent = entity_movement_trackers.get_or_insert_many(tracker_candidates.iter().map(
            |(motion, _)| {
                (
                    motion.id,
                    LastSentEntityState {
                        position: motion.position,
                        velocity: motion.velocity,
                        rotation: motion.rotation,
                        on_ground: motion.on_ground,
                        tracking_update_count: 0,
                        teleport_delay: 0,
                    },
                )
            },
        ));
        let tracker_inputs = tracker_candidates
            .into_iter()
            .zip(last_sent)
            .map(|((motion, already_scheduled), last_sent)| (motion, last_sent, already_scheduled))
            .collect::<Vec<_>>();
        #[cfg(feature = "load-bench")]
        {
            profile.record_filtered(state_count);
            profile.record_effective(state_count);
            profile.record_applied_steps(state_count);
            profile.record_tracker_inputs(tracker_inputs.len());
            profile.record_chunk_crossings(0);
        }
        drop(session_inner);
        #[cfg(feature = "load-bench")]
        profile.finish_locked_publish();
        let _ = self.dispatch_entity_movement_tracking(
            tick,
            Vec::new(),
            Vec::new(),
            tracker_inputs,
            movement_publication_budget,
            entity_movement_trackers,
            Vec::new(),
            HashMap::new(),
            #[cfg(feature = "load-bench")]
            &mut profile,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn dispatch_entity_movement_tracking(
        &self,
        tick: u64,
        handled_steps: Vec<EntityPhysicsStep>,
        mut dispatches: Vec<VisibilityDispatch>,
        tracker_inputs: Vec<(EntityTrackingMotion, LastSentEntityState, bool)>,
        movement_publication_budget: usize,
        entity_movement_trackers: Arc<EntityMovementTrackers>,
        pickup_sessions: Vec<u64>,
        old_observers_by_entity: HashMap<EntityId, HashSet<u64>>,
        #[cfg(feature = "load-bench")] profile: &mut PhysicsApplyProfile,
    ) -> Vec<EntityPhysicsStep> {
        if tracker_inputs.is_empty() {
            dispatches.extend(self.pickup_candidate_dispatches(pickup_sessions));
            dispatch_visibility_commands(dispatches);
            return handled_steps;
        }
        let ordinary_tracker_count = tracker_inputs
            .iter()
            .filter(|(motion, _, smooth_natural_mob)| {
                !(motion.is_arrow || motion.is_item || motion.is_experience || *smooth_natural_mob)
            })
            .count();

        dispatches.extend(self.pickup_candidate_dispatches(pickup_sessions));

        let mut movements =
            Vec::with_capacity(tracker_inputs.len().min(movement_publication_budget));
        let mut tracker_commits = Vec::with_capacity(tracker_inputs.len());
        let mut ordinary_ordinal = 0;
        for (motion, last_sent, smooth_natural_mob) in tracker_inputs {
            let latency_sensitive =
                motion.is_arrow || motion.is_item || motion.is_experience || smooth_natural_mob;
            if !latency_sensitive
                && !ordinary_entity_is_due_for_movement_tracking(
                    ordinary_ordinal,
                    tick,
                    ordinary_tracker_count,
                    movement_publication_budget,
                )
            {
                ordinary_ordinal += 1;
                continue;
            }
            ordinary_ordinal += usize::from(!latency_sensitive);
            let rotation = motion.rotation;
            let body_rotation_changed = packed_rotation_changed(last_sent.rotation, rotation);
            let send_head_rotation = packed_head_yaw_changed(last_sent.rotation, rotation);
            let send_velocity = motion.sends_velocity
                && entity_velocity_changed(last_sent.velocity, motion.velocity);
            let mut next_sent = last_sent;
            let position_update =
                plan_entity_position_update(&mut next_sent, motion.position, motion.on_ground);
            let wire_move = entity_wire_move_for_kind(
                position_update,
                body_rotation_changed,
                motion.position,
                motion.is_arrow,
            );
            if body_rotation_changed || position_update == EntityPositionUpdate::Absolute {
                next_sent.rotation.yaw = rotation.yaw;
                next_sent.rotation.pitch = rotation.pitch;
                next_sent.on_ground = motion.on_ground;
            }
            if send_head_rotation {
                next_sent.rotation.head_yaw = rotation.head_yaw;
            }
            if send_velocity {
                next_sent.velocity = motion.velocity;
            }
            tracker_commits.push((motion.id, last_sent, next_sent));
            if wire_move.is_none() && !send_velocity && !send_head_rotation {
                continue;
            }
            movements.push((
                motion.id,
                ServerEntityMove {
                    id: motion.id,
                    position: motion.position,
                    wire_move,
                    velocity: motion.velocity,
                    rotation,
                    on_ground: motion.on_ground,
                    send_velocity,
                    send_head_rotation,
                },
            ));
        }

        #[cfg(feature = "load-bench")]
        profile.record_movements(movements.len());
        if movements.is_empty() {
            entity_movement_trackers.compare_exchange_many(tracker_commits);
            dispatch_visibility_commands(dispatches);
            return handled_steps;
        }

        let recipient_index = self.movement_recipients.load_full();
        let recipient_snapshots = recipient_index
            .values()
            .map(|publication| {
                let visible_entities = publication.visible_entities();
                #[cfg(test)]
                self.pause_after_movement_visibility_load_for_test();
                (
                    publication.clone(),
                    publication.recipient(),
                    visible_entities,
                )
            })
            .collect::<Vec<_>>();
        let session_count = recipient_snapshots.len();
        let visibility_edge_count = recipient_snapshots
            .iter()
            .try_fold(0usize, |edge_count, (_, _, visible_entities)| {
                edge_count.checked_add(visible_entities.len())
            });
        let estimated_exhaustive_cost = session_count.saturating_mul(movements.len());
        // Building one flat sorted edge vector avoids one allocation per entity.
        let use_reverse_index = visibility_edge_count
            .is_some_and(|edge_count| estimated_exhaustive_cost > edge_count.saturating_mul(2));
        let mut movement_recipients = Vec::with_capacity(session_count);
        let mut current_observers_by_entity = None;
        if use_reverse_index {
            #[cfg(test)]
            record_movement_visibility_index_build();
            let mut reverse_index = Vec::with_capacity(visibility_edge_count.unwrap_or_default());
            for (publication, recipient, visible_entities) in recipient_snapshots {
                let recipient_index = movement_recipients.len();
                movement_recipients.push((
                    publication,
                    recipient,
                    Some(Arc::clone(&visible_entities)),
                ));
                for &entity_id in visible_entities.iter() {
                    #[cfg(test)]
                    record_movement_visibility_index_edge_visit();
                    reverse_index.push((entity_id, recipient_index));
                }
            }
            if Some(reverse_index.len()) == visibility_edge_count {
                reverse_index.sort_unstable();
                current_observers_by_entity = Some(reverse_index);
                for (_, _, visible_entities) in &mut movement_recipients {
                    *visible_entities = None;
                }
            }
        } else {
            movement_recipients.extend(recipient_snapshots.into_iter().map(
                |(publication, recipient, visible_entities)| {
                    (publication, recipient, Some(visible_entities))
                },
            ));
        }

        #[cfg(test)]
        self.pause_before_move_fanout_for_test();
        let mut movements_by_recipient = movement_recipients
            .into_iter()
            .map(|(publication, recipient, visible_entities)| {
                (publication, recipient, visible_entities, Vec::new())
            })
            .collect::<Vec<_>>();
        if let Some(current_observers_by_entity) = current_observers_by_entity.as_ref() {
            for (entity_id, movement) in &movements {
                let first = current_observers_by_entity
                    .partition_point(|(candidate, _)| candidate < entity_id);
                let after = current_observers_by_entity[first..]
                    .partition_point(|(candidate, _)| candidate == entity_id)
                    + first;
                for &(_, recipient_index) in &current_observers_by_entity[first..after] {
                    let (publication, _, _, recipient_movements) =
                        &mut movements_by_recipient[recipient_index];
                    if old_observers_by_entity
                        .get(entity_id)
                        .is_none_or(|observers| observers.contains(&publication.id()))
                    {
                        recipient_movements.push(*movement);
                    }
                }
            }
        } else {
            for (publication, _, visible_entities, recipient_movements) in
                &mut movements_by_recipient
            {
                let visible_entities = visible_entities
                    .as_ref()
                    .expect("exhaustive movement fanout retains current visibility");
                recipient_movements.extend(movements.iter().filter_map(|(entity_id, movement)| {
                    #[cfg(test)]
                    record_movement_exhaustive_membership_check();
                    (visible_entities.contains(entity_id)
                        && old_observers_by_entity
                            .get(entity_id)
                            .is_none_or(|observers| observers.contains(&publication.id())))
                    .then_some(*movement)
                }));
            }
        }
        let accepted_tracker_entities =
            entity_movement_trackers.compare_exchange_many(tracker_commits);
        let current_recipients = self.movement_recipients.load_full();
        let mut ordered_movements = Vec::with_capacity(movements_by_recipient.len());
        let mut canceled_recipients = Vec::new();
        let mut move_dispatch_count = 0usize;
        for (publication, recipient, _, mut movements) in movements_by_recipient {
            let Some(current_publication) = current_recipients.get(&publication.id()) else {
                canceled_recipients.push(recipient);
                continue;
            };
            if !publication.is_same_session(current_publication) {
                canceled_recipients.push(recipient);
                continue;
            }
            let visible_entities = publication.visible_entities();
            movements.retain(|movement| {
                accepted_tracker_entities.contains(&movement.id)
                    && visible_entities.contains(&movement.id)
            });
            if movements.is_empty() {
                canceled_recipients.push(recipient);
                continue;
            }
            move_dispatch_count += movements.len();
            ordered_movements.push((recipient, movements));
        }
        self.pressure_observation
            .record_unlocked_entity_move_dispatches(move_dispatch_count);
        drop(canceled_recipients);

        #[cfg(test)]
        self.pause_after_movement_recipient_validation_for_test();
        dispatch_visibility_commands(dispatches);
        for (recipient, mut movements) in ordered_movements {
            let command = if movements.len() == 1 {
                OutboundCommand::MoveEntityRelative(movements.pop().expect("one movement"))
            } else {
                OutboundCommand::MoveEntitiesRelative(movements)
            };
            dispatch_visibility_command(&recipient, command);
        }
        handled_steps
    }

    pub(crate) fn landed_falling_blocks(
        &self,
        expected: &[EntityPhysicsQuery],
        steps: &[EntityPhysicsStep],
    ) -> Vec<LandedFallingBlock> {
        let falling_block_ids = expected
            .iter()
            .filter(|query| query.kind == EntityPhysicsKind::FallingBlock)
            .map(|query| query.id)
            .collect::<HashSet<_>>();
        if falling_block_ids.is_empty() {
            return Vec::new();
        }
        let entities = self.lock_entities("collect landed falling blocks");
        steps
            .iter()
            .filter(|step| step.on_ground && falling_block_ids.contains(&step.id))
            .filter_map(|step| {
                let entity = entities.snapshot(step.id)?;
                if entity.lifecycle != EntityLifecycle::Alive
                    || entity.type_name != "minecraft:falling_block"
                {
                    return None;
                }
                let state = entity.block_state?;
                Some(LandedFallingBlock {
                    id: step.id,
                    pos: mc_world::BlockPos {
                        x: step.position.x.floor() as i32,
                        y: step.position.y.floor() as i32,
                        z: step.position.z.floor() as i32,
                    },
                    state: mc_world::BlockStateId(state),
                })
            })
            .collect()
    }

    pub(crate) fn remove_landed_falling_blocks(&self, ids: &[EntityId]) {
        if ids.is_empty() {
            return;
        }
        let mut inner = self.lock_session_entities("remove landed falling blocks");
        let dispatches = ids
            .iter()
            .filter_map(|id| {
                remove_server_entity_locked(&mut inner, *id).map(|(_, dispatches)| dispatches)
            })
            .flatten()
            .collect::<Vec<_>>();
        drop(inner);
        dispatch_visibility_commands(dispatches);
    }
}
