use super::entity_lifecycle::{
    remove_server_entity_locked, schedule_entity_death_locked, track_entity_chunk_locked,
    update_breeding_tick_tracking_locked,
};
use super::entity_physics_class::{
    entity_type_uses_aquatic_physics, entity_type_walks_on_powder_snow,
};
use super::explosion_authority::schedule_primed_tnt_deadline_locked;
use super::interaction_geometry::{
    distance_sq, entity_aabb, entity_geometry, entity_is_near_player_chunk,
};
use super::pickups::merge_item_entities_locked;
use super::simulation_input_publication::ExpectedEntityRoutingMove;
use super::visibility::{
    LastSentEntityState, entity_wire_move_for_kind, packed_head_yaw_changed,
    publish_server_entity_motion_locked,
};
use super::*;

mod persistence_projection;

use persistence_projection::{EntityPersistenceMetadata, project_owner_save};

fn entity_physics_query_matches(current: EntityMotionState, expected: &EntityPhysicsQuery) -> bool {
    let arrow_state_matches = match expected.kind {
        EntityPhysicsKind::ArrowProjectile {
            revision,
            embedded_block,
        } => {
            current.is_arrow
                && current.arrow_revision == revision
                && current.arrow_embedded_block == embedded_block
        }
        EntityPhysicsKind::Default
        | EntityPhysicsKind::Living
        | EntityPhysicsKind::PowderSnowWalkableLiving
        | EntityPhysicsKind::FallingBlock
        | EntityPhysicsKind::AquaticLiving => true,
    };
    current.position == expected.position
        && current.velocity == expected.velocity
        && current.on_ground == expected.on_ground
        && current.fall_distance == expected.fall_distance
        && arrow_state_matches
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
const VILLAGER_BRAIN_COMMIT_BATCH: usize = 64;
const VILLAGER_RESTOCK_REACH_SQUARED: f64 = 4.0;
const VILLAGER_GOSSIP_REACH_SQUARED: f64 = 5.0;
const VILLAGER_GOSSIP_COOLDOWN_TICKS: u64 = 1_200;
const VILLAGER_GOSSIP_CELL_SIZE: f64 = 3.0;

#[derive(Clone, Copy)]
struct VillagerProfessionContext<'a> {
    world_read: &'a mc_world::WorldReadView,
    blocks: &'a mc_world::BlockRegistry,
    items: &'a mc_data::items::ItemRegistry,
}

#[derive(Default)]
struct VillagerBrainTransitionReport {
    #[cfg(test)]
    applied: usize,
    metadata_updates: Vec<EntitySnapshot>,
}

fn current_villager_brain(
    entity: &EntitySnapshot,
) -> Option<mc_entity::villager_26_1_2::VillagerBrainState> {
    let villager = entity.retained.villager?;
    Some(entity.retained.villager_brain.clone().unwrap_or_else(|| {
        let job_site =
            (villager.profession != mc_entity::VillagerProfession::None).then_some(entity.position);
        mc_entity::villager_26_1_2::VillagerBrainState::adult(
            mc_entity::villager_26_1_2::VillagerPoiSet {
                home: Some(entity.position),
                job_site,
                meeting_point: Some(entity.position),
            },
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

fn supported_profession_assignment(
    entity: &EntitySnapshot,
    brain: &mc_entity::villager_26_1_2::VillagerBrainState,
    context: VillagerProfessionContext<'_>,
) -> Option<(
    mc_entity::VillagerData,
    mc_entity::villager_merchant_26_1_2::VillagerMerchantState,
)> {
    let villager = entity.retained.villager?;
    if villager.profession != mc_entity::VillagerProfession::None
        || villager.level != 1
        || entity.retained.villager_merchant.is_some()
        || brain.schedule != mc_entity::villager_26_1_2::VillagerScheduleKind::Adult
    {
        return None;
    }
    let job_site = brain.pois.job_site?;
    let state = context
        .world_read
        .get_cached_block(villager_job_site_block_pos(job_site))?;
    let block = &context.blocks.by_id(state)?.block.id;
    match mc_data::villager_trades_26_1_2::supported_profession_for_job_site_26_1_2(block)? {
        "toolsmith" => {
            let mut assigned = villager;
            assigned.profession = mc_entity::VillagerProfession::Toolsmith;
            Some((assigned, toolsmith_merchant_state(context.items)?))
        }
        _ => None,
    }
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

pub(super) fn villager_brain_due_for_tick(
    entity: EntityId,
    schedule: mc_entity::villager_26_1_2::VillagerScheduleKind,
    override_expires_tick: Option<u64>,
    lifecycle_tick: u64,
    day_time: i64,
    profile: &mc_entity::villager_26_1_2::VillagerBrainProfile,
) -> bool {
    override_expires_tick.is_some_and(|expires| lifecycle_tick >= expires)
        || villager_schedule_boundary(profile, schedule, day_time)
        || villager_brain_phase_due(entity, lifecycle_tick)
}

fn villager_can_restock_at_job_site(
    position: Vec3,
    brain: &mc_entity::villager_26_1_2::VillagerBrainState,
) -> bool {
    if brain.activity != mc_entity::villager_26_1_2::VillagerActivity::Work {
        return false;
    }
    let Some(job_site) = brain.pois.job_site else {
        return false;
    };
    let dx = position.x - job_site.x;
    let dy = position.y - job_site.y;
    let dz = position.z - job_site.z;
    dx * dx + dy * dy + dz * dz <= VILLAGER_RESTOCK_REACH_SQUARED
}

#[cfg(test)]
pub(super) fn apply_villager_brain_transitions(
    entities: &mut EntityStoreGuard<'_>,
    ids: &HashSet<EntityId>,
    lifecycle_tick: u64,
    day_time: i64,
    profile: &mc_entity::villager_26_1_2::VillagerBrainProfile,
) -> usize {
    apply_villager_brain_transitions_with_professions(
        entities,
        ids,
        lifecycle_tick,
        day_time,
        profile,
        None,
    )
    .applied
}

fn apply_villager_brain_transitions_with_professions(
    entities: &mut EntityStoreGuard<'_>,
    ids: &HashSet<EntityId>,
    lifecycle_tick: u64,
    day_time: i64,
    profile: &mc_entity::villager_26_1_2::VillagerBrainProfile,
    profession_context: Option<VillagerProfessionContext<'_>>,
) -> VillagerBrainTransitionReport {
    if ids.is_empty() {
        return VillagerBrainTransitionReport::default();
    }
    let Ok(validated_profile) = profile.validated() else {
        return VillagerBrainTransitionReport::default();
    };
    let mut ordered = ids.iter().copied().collect::<Vec<_>>();
    ordered.sort_unstable();
    let mut transitions = Vec::new();
    let mut expected_metadata = Vec::new();
    for id in ordered {
        let Some(expected) = entities.snapshot(id) else {
            continue;
        };
        if expected.lifecycle != EntityLifecycle::Alive
            || expected.type_name != "minecraft:villager"
        {
            continue;
        }
        let Some(current) = current_villager_brain(&expected) else {
            continue;
        };
        let Ok(plan) = validated_profile.plan(&current, lifecycle_tick, day_time) else {
            continue;
        };
        let profession_assignment = profession_context
            .and_then(|context| supported_profession_assignment(&expected, &plan.state, context));
        let updated_gossip = expected
            .retained
            .villager_gossip
            .clone()
            .and_then(|mut gossip| (gossip.decay(day_time).ok() == Some(true)).then_some(gossip));
        let updated_merchant =
            expected
                .retained
                .villager_merchant
                .clone()
                .and_then(|mut merchant| {
                    (villager_can_restock_at_job_site(expected.position, &plan.state)
                        && merchant.restock(day_time).ok() == Some(true))
                    .then_some(merchant)
                });
        if expected.goal == plan.goal
            && expected.retained.villager_brain.as_ref() == Some(&plan.state)
            && updated_gossip.is_none()
            && updated_merchant.is_none()
            && profession_assignment.is_none()
        {
            continue;
        }
        let mut next = expected.clone();
        next.goal = plan.goal;
        next.retained.villager_brain = Some(plan.state);
        if let Some(gossip) = updated_gossip {
            next.retained.villager_gossip = Some(gossip);
        }
        if let Some(merchant) = updated_merchant {
            next.retained.villager_merchant = Some(merchant);
        }
        if let Some((assigned, merchant)) = profession_assignment {
            next.retained.villager = Some(assigned);
            next.retained.villager_merchant = Some(merchant.clone());
            expected_metadata.push((next.id, assigned, merchant));
        }
        transitions.push((expected, next));
    }
    let _applied = commit_villager_brain_transitions(entities, transitions);
    let metadata_updates = expected_metadata
        .into_iter()
        .filter_map(|(id, expected_villager, expected_merchant)| {
            let current = entities.snapshot(id)?;
            (current.retained.villager == Some(expected_villager)
                && current.retained.villager_merchant.as_ref() == Some(&expected_merchant))
            .then_some(current)
        })
        .collect();
    VillagerBrainTransitionReport {
        #[cfg(test)]
        applied: _applied,
        metadata_updates,
    }
}

pub(super) fn commit_villager_brain_transitions(
    entities: &mut EntityStoreGuard<'_>,
    transitions: Vec<(EntitySnapshot, EntitySnapshot)>,
) -> usize {
    let mut transitions = transitions.into_iter();
    let mut applied = 0;
    loop {
        let batch = transitions
            .by_ref()
            .take(VILLAGER_BRAIN_COMMIT_BATCH)
            .collect::<Vec<_>>();
        if batch.is_empty() {
            return applied;
        }
        applied += commit_villager_brain_transition_batch(entities, batch);
    }
}

fn commit_villager_brain_transition_batch(
    entities: &mut EntityStoreGuard<'_>,
    mut batch: Vec<(EntitySnapshot, EntitySnapshot)>,
) -> usize {
    let count = batch.len();
    if entities.replace_snapshots_if_current(batch.iter().cloned()) {
        return count;
    }
    if count <= 1 {
        return 0;
    }
    let right = batch.split_off(count / 2);
    commit_villager_brain_transition_batch(entities, batch)
        + commit_villager_brain_transition_batch(entities, right)
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
) -> Option<EntityId> {
    let eligible = |target: EntityId| {
        if target == receiver.id || reserved.contains(&target) {
            return None;
        }
        let snapshot = candidates.get(&target)?;
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

pub(super) fn apply_villager_gossip_transfers(
    entities: &mut EntityStoreGuard<'_>,
    initiator_ids: &HashSet<EntityId>,
    candidate_ids: &HashSet<EntityId>,
    timestamp: u64,
) -> usize {
    if initiator_ids.is_empty() || candidate_ids.len() < 2 {
        return 0;
    }
    let mut candidates = HashMap::<EntityId, EntitySnapshot>::new();
    let mut cells = HashMap::<(i32, i32, i32), Vec<EntityId>>::new();
    let mut ordered_candidates = candidate_ids.iter().copied().collect::<Vec<_>>();
    ordered_candidates.sort_unstable();
    for id in ordered_candidates {
        let Some(snapshot) = entities.snapshot(id) else {
            continue;
        };
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

    pub(in crate::play) fn tick_entities_and_collect_physics_queries_owned(
        &self,
        _authority: &SimulationAuthority,
        cpu_resources: &crate::chunk_pipeline::ChunkPipelineResources,
        tick: u64,
        pathing_candidates_per_entity: usize,
        simulation_distance: i32,
        world: EntitySimulationWorldContext<'_>,
    ) -> Vec<EntityPhysicsQuery> {
        self.tick_entities_and_collect_physics_queries_core(
            Some(cpu_resources),
            tick,
            pathing_candidates_per_entity,
            simulation_distance,
            world.pathing(),
            world.profession_context(),
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
            PathingBudget::DEFAULT.max_candidates_per_entity,
            DEFAULT_VIEW_DISTANCE,
            None,
            None,
        )
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
            PathingBudget::DEFAULT.max_candidates_per_entity,
            simulation_distance,
            None,
            None,
        )
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
            pathing_candidates_per_entity,
            DEFAULT_VIEW_DISTANCE,
            None,
            None,
        )
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
            PathingBudget::DEFAULT.max_candidates_per_entity,
            DEFAULT_VIEW_DISTANCE,
            Some((world_read, pathing_materials)),
            None,
        )
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
            PathingBudget::DEFAULT.max_candidates_per_entity,
            DEFAULT_VIEW_DISTANCE,
            None,
            Some((world_read, blocks, items)),
        )
    }

    fn tick_entities_and_collect_physics_queries_core(
        &self,
        cpu_resources: Option<&crate::chunk_pipeline::ChunkPipelineResources>,
        tick: u64,
        pathing_candidates_per_entity: usize,
        simulation_distance: i32,
        pathing: Option<(&mc_world::WorldReadView, &mc_physics::BlockMaterialIds)>,
        profession_context: Option<(
            &mc_world::WorldReadView,
            &mc_world::BlockRegistry,
            &mc_data::items::ItemRegistry,
        )>,
    ) -> Vec<EntityPhysicsQuery> {
        if !self.has_live_sessions() {
            self.clear_active_simulation_entities();
            return Vec::new();
        }
        let live_session_generation = self.live_session_generation.load(Ordering::Acquire);
        let (world_read, pathing_materials) = pathing.unzip();
        let profession_context =
            profession_context.map(|(world_read, blocks, items)| VillagerProfessionContext {
                world_read,
                blocks,
                items,
            });
        let (active_chunks, active_entity_candidates) =
            self.simulation_inputs.active_entity_candidates();
        let recipients = self.movement_recipients.load_full();
        let mut player_positions = Vec::new();
        let mut hostile_target_positions = Vec::new();
        for publication in recipients.values() {
            let target = *publication.combat_target();
            let position = Vec3::new(target.pose().x, target.pose().y, target.pose().z);
            if target.is_alive() {
                player_positions.push(position);
            }
            if target.is_targetable() {
                hostile_target_positions.push(position);
            }
        }
        let terrain_pathing_entities = self.simulation_inputs.terrain_pathing_entities();
        let active_population_ids = active_entity_candidates
            .into_iter()
            .filter(|&entity| {
                self.simulation_inputs
                    .entity_chunk(entity)
                    .is_some_and(|chunk| {
                        active_chunks.contains(&chunk)
                            && entity_is_near_player_chunk(
                                chunk,
                                &player_positions,
                                simulation_distance,
                            )
                    })
            })
            .collect::<HashSet<_>>();
        let villager_day_time = i64::try_from(self.world_time()).unwrap_or(i64::MAX);
        let villager_profile = self.villager_brain_profile();
        let overridden_villagers = self.overridden_villager_entities();
        let villager_brain_probe_ids = villager_brain_probe_ids(
            &active_population_ids,
            &overridden_villagers,
            tick,
            villager_day_time,
            &villager_profile,
        );
        let simulation_budget = cpu_resources.map_or(usize::MAX, |cpu| {
            cpu.cpu_limit()
                .max(1)
                .saturating_mul(ENTITY_SIMULATION_UPDATES_PER_LANE_PER_TICK)
        });
        let simulation_overloaded = active_population_ids.len() > simulation_budget;
        let active_entity_candidates = if simulation_overloaded {
            bounded_entity_ids_due_for_tick(&active_population_ids, tick, simulation_budget)
        } else {
            active_population_ids.clone()
        };
        let mut entities = self.lock_entities("prepare entity goals");
        if active_chunks.is_empty() {
            self.clear_active_simulation_entities();
            return Vec::new();
        }
        let mut active_entity_ids = HashSet::new();
        let mut active_hostile_ids = HashSet::new();
        let mut active_villager_ids = HashSet::new();
        let mut active_villager_population_ids = HashSet::new();
        let mut sheep_grazing_entities = HashSet::new();
        let mut active_entity_aabbs = HashMap::new();
        let mut active_entity_kinds = HashMap::new();
        entities.visit_simulation_entities_for_ids(&active_entity_candidates, |entity| {
            #[cfg(test)]
            self.active_entity_selection_visits
                .fetch_add(1, Ordering::Relaxed);
            if entity.lifecycle == EntityLifecycle::Alive {
                let chunk = chunk_pos_from_coords(entity.position.x, entity.position.z);
                if active_chunks.contains(&chunk) {
                    active_entity_ids.insert(entity.id);
                    if is_hostile_entity(entity.type_name) {
                        active_hostile_ids.insert(entity.id);
                    }
                    if entity.type_name == "minecraft:villager"
                        && entity.retained.villager.is_some()
                    {
                        active_villager_population_ids.insert(entity.id);
                    }
                    if entity.retained.sheep_grazing_ticks.is_some() {
                        sheep_grazing_entities.insert(entity.id);
                    }
                    active_entity_aabbs.insert(
                        entity.id,
                        entity_geometry(entity.type_name, entity.animal).aabb,
                    );
                    active_entity_kinds.insert(
                        entity.id,
                        (
                            if entity.type_name == "minecraft:arrow" {
                                EntityPhysicsKind::ArrowProjectile {
                                    revision: entity
                                        .retained
                                        .arrow_state
                                        .map(|state| state.projectile.revision),
                                    embedded_block: entity
                                        .retained
                                        .arrow_state
                                        .filter(|state| state.in_ground)
                                        .and_then(|state| state.last_block_position),
                                }
                            } else if entity_type_uses_aquatic_physics(entity.type_name) {
                                EntityPhysicsKind::AquaticLiving
                            } else if entity.type_name == "minecraft:falling_block" {
                                EntityPhysicsKind::FallingBlock
                            } else if entity.item_stack.is_none()
                                && entity.experience_value.is_none()
                                && entity.block_state.is_none()
                                && entity.vehicle.is_none()
                            {
                                if entity_type_walks_on_powder_snow(entity.type_name) {
                                    EntityPhysicsKind::PowderSnowWalkableLiving
                                } else {
                                    EntityPhysicsKind::Living
                                }
                            } else {
                                EntityPhysicsKind::Default
                            },
                            entity.retained.fall_distance,
                        ),
                    );
                }
            }
        });
        entities.visit_simulation_entities_for_ids(&villager_brain_probe_ids, |entity| {
            if entity.lifecycle == EntityLifecycle::Alive
                && entity.type_name == "minecraft:villager"
                && entity.retained.villager.is_some()
                && villager_brain_due_for_tick(
                    entity.id,
                    entity.retained.villager_brain.as_ref().map_or(
                        mc_entity::villager_26_1_2::VillagerScheduleKind::Adult,
                        |brain| brain.schedule,
                    ),
                    entity
                        .retained
                        .villager_brain
                        .as_ref()
                        .and_then(|brain| brain.override_expires_tick),
                    tick,
                    villager_day_time,
                    &villager_profile,
                )
            {
                active_villager_ids.insert(entity.id);
            }
        });
        let VillagerBrainTransitionReport {
            metadata_updates: villager_metadata_updates,
            ..
        } = apply_villager_brain_transitions_with_professions(
            &mut entities,
            &active_villager_ids,
            tick,
            villager_day_time,
            &villager_profile,
            profession_context,
        );
        let _gossip_transfers = apply_villager_gossip_transfers(
            &mut entities,
            &active_villager_ids,
            &active_villager_population_ids,
            tick,
        );
        let cleared_overrides = overridden_villagers
            .iter()
            .copied()
            .filter(|entity| {
                entities.snapshot(*entity).is_none_or(|snapshot| {
                    snapshot.lifecycle != EntityLifecycle::Alive
                        || snapshot
                            .retained
                            .villager_brain
                            .as_ref()
                            .is_none_or(|brain| brain.override_order.is_none())
                })
            })
            .collect::<Vec<_>>();
        self.clear_villager_overrides(&cleared_overrides);
        if active_entity_ids.is_empty() {
            self.publish_active_entity_selection(
                live_session_generation,
                active_population_ids,
                active_hostile_ids,
            );
            drop(entities);
            self.publish_villager_metadata_updates(villager_metadata_updates);
            return Vec::new();
        }
        let mob_behaviors = self.mob_behavior_table();
        update_hostile_targets(
            &mut entities,
            &hostile_target_positions,
            Some(&active_hostile_ids),
            &mob_behaviors,
        );
        self.publish_active_entity_selection(
            live_session_generation,
            active_population_ids,
            active_hostile_ids,
        );
        let eligible_goal_entity_ids = active_entity_ids
            .difference(&sheep_grazing_entities)
            .copied()
            .collect::<HashSet<_>>();
        let goal_entity_ids =
            entity_goal_ids_due_for_tick(&eligible_goal_entity_ids, tick, simulation_overloaded);
        let unprojected_entity_ids = active_entity_ids
            .difference(&goal_entity_ids)
            .copied()
            .collect::<HashSet<_>>();
        let prepared_goal_tick =
            entities.prepare_goal_tick_with_pathing_for_ids(tick, &goal_entity_ids);
        drop(entities);
        self.publish_villager_metadata_updates(villager_metadata_updates);
        #[cfg(test)]
        self.pause_before_entity_goal_compute_for_test();
        let goal_budget = PathingBudget {
            max_candidates_per_entity: pathing_candidates_per_entity.max(1),
            ..PathingBudget::DEFAULT
        };
        let terrain_snapshot = if terrain_pathing_entities.is_empty() {
            None
        } else {
            world_read.zip(pathing_materials).map(|(world_read, _)| {
                let mut chunks = HashSet::new();
                prepared_goal_tick.visit_pathing_probe_positions(
                    goal_budget,
                    |entity, position| {
                        insert_terrain_snapshot_chunks_for_probe_position(
                            &mut chunks,
                            entity,
                            position,
                            &terrain_pathing_entities,
                            &active_entity_aabbs,
                            &active_chunks,
                        );
                    },
                );
                let chunks = sorted_chunk_positions(chunks);
                world_read.snapshot_chunks(&chunks)
            })
        };
        let pathing_probe = LoadedChunkPathingProbe::new(
            &active_chunks,
            &terrain_pathing_entities,
            &active_entity_aabbs,
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
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut entities = self.lock_entities("apply entity goals");
        let applied = entities
            .apply_prepared_goal_tick_and_alive_kinematics(resolved_goal_tick, &goal_entity_ids);
        let current_ids = if applied.is_some() {
            for id in &unprojected_entity_ids {
                active_entity_aabbs.remove(id);
                active_entity_kinds.remove(id);
            }
            &unprojected_entity_ids
        } else {
            active_entity_aabbs.clear();
            active_entity_kinds.clear();
            &active_entity_ids
        };
        let mut kinematics = applied
            .map(|(_, kinematics)| kinematics)
            .unwrap_or_default();
        entities.visit_simulation_entities_for_ids(current_ids, |entity| {
            if entity.lifecycle != EntityLifecycle::Alive {
                return;
            }
            let chunk = chunk_pos_from_coords(entity.position.x, entity.position.z);
            if !active_chunks.contains(&chunk)
                || !entity_is_near_player_chunk(chunk, &player_positions, simulation_distance)
            {
                return;
            }
            active_entity_aabbs.insert(
                entity.id,
                entity_geometry(entity.type_name, entity.animal).aabb,
            );
            active_entity_kinds.insert(
                entity.id,
                (
                    if entity.type_name == "minecraft:arrow" {
                        EntityPhysicsKind::ArrowProjectile {
                            revision: entity
                                .retained
                                .arrow_state
                                .map(|state| state.projectile.revision),
                            embedded_block: entity
                                .retained
                                .arrow_state
                                .filter(|state| state.in_ground)
                                .and_then(|state| state.last_block_position),
                        }
                    } else if entity_type_uses_aquatic_physics(entity.type_name) {
                        EntityPhysicsKind::AquaticLiving
                    } else if entity.type_name == "minecraft:falling_block" {
                        EntityPhysicsKind::FallingBlock
                    } else if entity.item_stack.is_none()
                        && entity.experience_value.is_none()
                        && entity.block_state.is_none()
                        && entity.vehicle.is_none()
                    {
                        if entity_type_walks_on_powder_snow(entity.type_name) {
                            EntityPhysicsKind::PowderSnowWalkableLiving
                        } else {
                            EntityPhysicsKind::Living
                        }
                    } else {
                        EntityPhysicsKind::Default
                    },
                    entity.retained.fall_distance,
                ),
            );
            kinematics.push(EntityKinematics {
                id: entity.id,
                position: entity.position,
                rotation: entity.rotation,
                velocity: entity.velocity,
                on_ground: entity.on_ground,
            });
        });
        kinematics.sort_unstable_by_key(|state| state.id);
        drop(goal_cpu_permits);
        let queries = kinematics
            .into_iter()
            .map(|state| EntityPhysicsQuery {
                id: state.id,
                position: state.position,
                velocity: state.velocity,
                aabb: active_entity_aabbs[&state.id],
                on_ground: state.on_ground,
                fall_distance: active_entity_kinds[&state.id].1,
                kind: active_entity_kinds[&state.id].0,
            })
            .collect();
        drop(entities);
        if !resolved_direct_paths.is_empty() {
            self.simulation_inputs
                .remove_terrain_pathing(resolved_direct_paths);
        }
        queries
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
            if is_hostile_entity(&entity.type_name) {
                inner.hostile_entities.insert(entity_id);
                inner.natural_hostile_mobs.insert(entity_id);
            } else if entity_type_uses_aquatic_physics(&entity.type_name) {
                inner.natural_aquatic_mobs.insert(entity_id);
            } else if entity.animal.is_some() {
                inner.natural_ground_mobs.insert(entity_id);
            }
            if entity.type_name == "minecraft:sheep" {
                inner.sheep_entities.insert(entity_id);
            }
            update_breeding_tick_tracking_locked(&mut inner, entity_id, entity.animal);
            schedule_entity_death_locked(&mut inner, &entity);
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
        let saved = owner_result(self.entities.handle.save_barrier());
        let (mut checkpoint, phases) = project_owner_save(saved, &metadata);
        checkpoint.settlement_claims = self
            .lock_inner("snapshot settlement spawn claims")
            .settlement_spawn_claims
            .clone();
        (checkpoint, phases)
    }

    #[cfg(test)]
    pub(in crate::play) fn apply_entity_physics_and_dispatch_owned(
        &self,
        _authority: &SimulationAuthority,
        tick: u64,
        steps: &[EntityPhysicsStep],
    ) {
        let arrow_physics_facts = test_arrow_physics_facts(steps);
        let _ = self.apply_entity_physics_and_dispatch_core(
            None,
            tick,
            None,
            steps,
            &arrow_physics_facts,
        );
    }

    pub(in crate::play) fn apply_entity_physics_if_current_and_dispatch_owned(
        &self,
        _authority: &SimulationAuthority,
        cpu_resources: &crate::chunk_pipeline::ChunkPipelineResources,
        tick: u64,
        expected: &[EntityPhysicsQuery],
        steps: &[EntityPhysicsStep],
        arrow_physics_facts: &[ArrowPhysicsFact],
    ) -> Vec<EntityPhysicsStep> {
        self.apply_entity_physics_and_dispatch_core(
            Some(cpu_resources),
            tick,
            Some(expected),
            steps,
            arrow_physics_facts,
        )
    }

    #[cfg(test)]
    pub(crate) fn apply_entity_physics_and_dispatch(&self, tick: u64, steps: &[EntityPhysicsStep]) {
        let arrow_physics_facts = test_arrow_physics_facts(steps);
        let _ = self.apply_entity_physics_and_dispatch_core(
            None,
            tick,
            None,
            steps,
            &arrow_physics_facts,
        );
    }

    #[cfg(test)]
    pub(crate) fn apply_entity_physics_with_arrow_facts_and_dispatch(
        &self,
        tick: u64,
        steps: &[EntityPhysicsStep],
        arrow_physics_facts: &[ArrowPhysicsFact],
    ) {
        let _ = self.apply_entity_physics_and_dispatch_core(
            None,
            tick,
            None,
            steps,
            arrow_physics_facts,
        );
    }

    #[cfg(test)]
    pub(crate) fn apply_entity_physics_if_current_and_dispatch(
        &self,
        tick: u64,
        expected: &[EntityPhysicsQuery],
        steps: &[EntityPhysicsStep],
    ) {
        let arrow_physics_facts = test_arrow_physics_facts(steps);
        let _ = self.apply_entity_physics_and_dispatch_core(
            None,
            tick,
            Some(expected),
            steps,
            &arrow_physics_facts,
        );
    }

    pub(super) fn apply_entity_physics_and_dispatch_core(
        &self,
        cpu_resources: Option<&crate::chunk_pipeline::ChunkPipelineResources>,
        tick: u64,
        expected: Option<&[EntityPhysicsQuery]>,
        steps: &[EntityPhysicsStep],
        arrow_physics_facts: &[ArrowPhysicsFact],
    ) -> Vec<EntityPhysicsStep> {
        let mut scheduled_tracker_ids = steps.iter().map(|step| step.id).collect::<Vec<_>>();
        scheduled_tracker_ids.sort_unstable();
        scheduled_tracker_ids.dedup();
        let step_ids = scheduled_tracker_ids
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        let mut entities = self.lock_entities("prepare entity physics");
        entities.prefetch(&step_ids);
        self.entity_lifecycle_tick.fetch_max(tick, Ordering::AcqRel);
        let expected_by_id = expected.map(|queries| {
            queries
                .iter()
                .map(|query| (query.id, *query))
                .collect::<HashMap<_, _>>()
        });
        let needs_filter = steps.iter().any(|step| {
            if !step.position.is_finite() || !step.velocity.is_finite() {
                return true;
            }
            let Some(expected_by_id) = expected_by_id.as_ref() else {
                return false;
            };
            let Some(expected) = expected_by_id.get(&step.id) else {
                return true;
            };
            !entities
                .motion_state(step.id)
                .is_some_and(|current| entity_physics_query_matches(current, expected))
        });
        let filtered_steps = needs_filter.then(|| {
            steps
                .iter()
                .copied()
                .filter(|step| {
                    if !step.position.is_finite() || !step.velocity.is_finite() {
                        return false;
                    }
                    let Some(expected_by_id) = expected_by_id.as_ref() else {
                        return true;
                    };
                    let Some(expected) = expected_by_id.get(&step.id) else {
                        return false;
                    };
                    entities
                        .motion_state(step.id)
                        .is_some_and(|current| entity_physics_query_matches(current, expected))
                })
                .collect::<Vec<_>>()
        });
        let steps = filtered_steps.as_deref().unwrap_or(steps);
        let old_chunks: HashMap<_, _> = steps
            .iter()
            .filter_map(|step| {
                self.simulation_inputs
                    .entity_chunk(step.id)
                    .map(|chunk| (step.id, chunk))
            })
            .collect();
        let old_motion: HashMap<_, _> = steps
            .iter()
            .filter_map(|step| entities.motion_state(step.id).map(|state| (step.id, state)))
            .collect();
        let kinematics = steps
            .iter()
            .filter_map(|step| {
                old_motion
                    .get(&step.id)
                    .filter(|motion| !motion.is_arrow)
                    .map(|motion| EntityKinematics {
                        id: step.id,
                        position: step.position,
                        rotation: motion.rotation,
                        velocity: step.velocity,
                        on_ground: step.on_ground,
                    })
            })
            .collect::<Vec<_>>();
        let regional_batch_count = entities.parallel_kinematics_batch_count(&kinematics);
        let regional_worker_permits = cpu_resources
            .map(|resources| acquire_regional_worker_permits(resources, regional_batch_count))
            .unwrap_or_default();
        #[cfg(test)]
        self.pause_before_physics_owner_apply_for_test();
        let applied_kinematics = if regional_worker_permits.is_empty() {
            entities.apply_kinematics_authoritative(kinematics)
        } else {
            entities.apply_kinematics_parallel_authoritative(
                kinematics,
                regional_worker_permits.len() + 1,
            )
        };
        drop(regional_worker_permits);
        let session_inner = self.lock_inner("publish entity physics");
        // Re-read under the publication lock. Regional owner mutation does not
        // require this lock, so a snapshot taken while waiting for it is not a
        // valid publication fence.
        for id in &step_ids {
            entities.invalidate(*id);
        }
        entities.prefetch(&step_ids);
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
                let current = inner.entities.motion_state(state.id)?;
                (current.position == state.position
                    && current.rotation == state.rotation
                    && current.velocity == state.velocity
                    && current.on_ground == state.on_ground)
                    .then_some((state, current))
            })
            .collect::<Vec<_>>();
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
        for (_, motion) in applied_motion {
            publish_server_entity_motion_locked(&mut inner, motion);
        }
        inner = resolve_arrow_entity_hits_locked(
            self,
            inner,
            steps,
            &old_motion,
            arrow_physics_facts,
            &mut dispatches,
        );
        let mut rejected_arrows = std::mem::take(&mut inner.arrow_tick_scratch.rejected);
        let mut processed_arrows = std::mem::take(&mut inner.arrow_tick_scratch.processed);
        applied_steps.extend(steps.iter().copied().filter(|step| {
            processed_arrows.contains(&step.id)
                && old_motion
                    .get(&step.id)
                    .is_some_and(|motion| motion.is_arrow)
        }));
        let effective_steps = applied_steps
            .iter()
            .filter(|step| !rejected_arrows.contains(&step.id))
            .filter_map(|step| {
                let motion = inner.entities.motion_state(step.id)?;
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
            dispatches.extend(refresh_entity_target_visibility_locked(
                &mut inner, entity_id, old_chunk, new_chunk,
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
        let natural_tracker_ids = steps
            .iter()
            .filter(|step| {
                inner.natural_hostile_mobs.contains(&step.id)
                    || inner.natural_ground_mobs.contains(&step.id)
                    || inner.natural_aquatic_mobs.contains(&step.id)
            })
            .map(|step| step.id)
            .collect::<HashSet<_>>();
        let natural_tracker_ids_due = bounded_entity_ids_due_for_tick(
            &natural_tracker_ids,
            tick,
            ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN,
        );
        let mut tracker_inputs = Vec::with_capacity(steps.len());
        for step in steps {
            let Some(motion) = inner.entities.motion_state(step.id) else {
                continue;
            };
            let latency_sensitive = motion.is_arrow || motion.is_item || motion.is_experience;
            let smooth_natural_mob = inner.natural_hostile_mobs.contains(&step.id)
                || inner.natural_ground_mobs.contains(&step.id)
                || inner.natural_aquatic_mobs.contains(&step.id);
            if smooth_natural_mob && !natural_tracker_ids_due.contains(&step.id) {
                continue;
            }
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
            tracker_inputs.push((motion, last_sent, smooth_natural_mob));
        }
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
        if tracker_inputs.is_empty() {
            dispatches.extend(self.pickup_candidate_dispatches(pickup_sessions));
            dispatch_visibility_commands(dispatches);
            return steps.to_vec();
        }
        let ordinary_tracker_count = tracker_inputs
            .iter()
            .filter(|(motion, _, smooth_natural_mob)| {
                !(motion.is_arrow || motion.is_item || motion.is_experience || *smooth_natural_mob)
            })
            .count();

        dispatches.extend(self.pickup_candidate_dispatches(pickup_sessions));

        let mut movements = Vec::with_capacity(
            tracker_inputs
                .len()
                .min(ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN),
        );
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

        if movements.is_empty() {
            entity_movement_trackers.compare_exchange_many(tracker_commits);
            dispatch_visibility_commands(dispatches);
            return steps.to_vec();
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
        // Charge one extra unit per edge for reverse-map allocation and insertion.
        let use_reverse_index = visibility_edge_count
            .is_some_and(|edge_count| estimated_exhaustive_cost > edge_count.saturating_mul(2));
        let mut movement_recipients = Vec::with_capacity(session_count);
        let mut current_observers_by_entity = None;
        if use_reverse_index {
            #[cfg(test)]
            record_movement_visibility_index_build();
            let mut reverse_index = HashMap::<EntityId, Vec<usize>>::new();
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
                    reverse_index
                        .entry(entity_id)
                        .or_default()
                        .push(recipient_index);
                }
            }
            let indexed_visibility_edges = reverse_index
                .values()
                .try_fold(0usize, |edge_count, observer_indexes| {
                    edge_count.checked_add(observer_indexes.len())
                });
            if indexed_visibility_edges == visibility_edge_count {
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
                let Some(candidate_indexes) = current_observers_by_entity.get(entity_id) else {
                    continue;
                };
                for &recipient_index in candidate_indexes {
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
        steps.to_vec()
    }

    pub(crate) fn landed_falling_blocks(
        &self,
        steps: &[EntityPhysicsStep],
    ) -> Vec<LandedFallingBlock> {
        let entities = self.lock_entities("collect landed falling blocks");
        steps
            .iter()
            .filter(|step| step.on_ground)
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
