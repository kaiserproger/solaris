use mc_entity::{EntityId, EntityLifecycle, EntitySnapshot, SpawnEntity, Vec3};
use mc_world::BlockStateId;
use std::sync::atomic::Ordering;

use crate::play::is_hostile_entity;
use crate::play::simulation::SimulationAuthority;
use crate::play::spawn::chunk_pos_from_coords;

use super::entity_goal_defaults::apply_default_mob_goal;
use super::interaction_geometry::{distance_sq, entity_aabb};
use super::outbound::{ServerEntitySnapshot, VisibilityDispatch};
use super::visibility::{
    clear_entity_publication_state_locked, despawn_entity_visibility_locked,
    entity_event_dispatches_locked, initialize_entity_wire_state_locked,
    server_entity_snapshot_from, spawn_entity_visibility_locked,
};
use super::{SessionEntityGuards, SessionRegistry, SessionRegistryInner, apply_entity_facts};

pub(super) const ENTITY_EVENT_DEATH_COMPLETE: i8 = 60;
pub(super) const DEATH_REMOVALS_PER_TICK: usize = 4;
const NATURAL_MOB_SOFT_DESPAWN_IDLE_TICKS: u64 = 600;
const NATURAL_MOB_SOFT_DESPAWN_ROLL_BOUND: u32 = 800;
const JAVA_RANDOM_MULTIPLIER: u64 = 0x5DEECE66D;
const JAVA_RANDOM_ADDEND: u64 = 0xB;
const JAVA_RANDOM_MASK: u64 = (1_u64 << 48) - 1;

pub(crate) struct NaturalMobDespawnOutcome {
    #[cfg(test)]
    pub(super) removed: usize,
    pub(super) dispatches: Vec<VisibilityDispatch>,
}

#[cfg(test)]
impl NaturalMobDespawnOutcome {
    pub(crate) const fn removed(&self) -> usize {
        self.removed
    }
}

impl SessionRegistry {
    pub(crate) fn synchronize_entity_lifecycle_epoch(&self, lifecycle_epoch: u64) {
        let previous = self
            .entity_lifecycle_tick
            .fetch_max(lifecycle_epoch, Ordering::AcqRel);
        let lifecycle_epoch = previous.max(lifecycle_epoch);
        if lifecycle_epoch != previous {
            self.simulation_tick_sender.send_replace(lifecycle_epoch);
        }
        self.entities.advance_lifecycle_epoch(lifecycle_epoch);
    }

    pub(crate) fn tick_natural_mob_despawn(&self, current_tick: u64) -> NaturalMobDespawnOutcome {
        let mut inner = self.lock_session_entities("despawn distant natural mobs");
        let player_positions = inner
            .sessions
            .iter()
            .filter(|(id, _)| {
                !inner.dead_sessions.contains(id)
                    && !inner.spectator_sessions.contains(id)
                    && !inner.client_unloaded_sessions.contains(id)
            })
            .map(|(_, session)| Vec3::new(session.pose.x, session.pose.y, session.pose.z))
            .collect::<Vec<_>>();
        if player_positions.is_empty() {
            return NaturalMobDespawnOutcome {
                #[cfg(test)]
                removed: 0,
                dispatches: Vec::new(),
            };
        }
        let mut candidates = inner
            .natural_hostile_mobs
            .iter()
            .chain(&inner.natural_ground_mobs)
            .chain(&inner.natural_aquatic_mobs)
            .copied()
            .collect::<Vec<_>>();
        candidates.sort_unstable();
        candidates.dedup();
        let mut dispatches = Vec::new();
        #[cfg(test)]
        let mut removed = 0usize;
        for entity_id in candidates {
            let Some(snapshot) = inner.entities.snapshot(entity_id) else {
                inner.natural_mob_no_action_since_tick.remove(&entity_id);
                continue;
            };
            if snapshot.lifecycle != EntityLifecycle::Alive {
                continue;
            }
            let Some(contract) =
                mc_data::entity_types::entity_type_contract_26_1_2_by_name(&snapshot.type_name)
            else {
                continue;
            };
            if !natural_mob_remove_when_far_away(&snapshot.type_name) {
                inner
                    .natural_mob_no_action_since_tick
                    .insert(entity_id, current_tick);
                continue;
            }
            let nearest_distance_sq = player_positions
                .iter()
                .map(|player| distance_sq(snapshot.position, *player))
                .min_by(f64::total_cmp)
                .expect("non-empty player positions");
            let category = contract.mob_category();
            let hard_distance = f64::from(category.despawn_distance());
            let should_hard_despawn = nearest_distance_sq > hard_distance * hard_distance;

            let no_despawn_distance = f64::from(category.no_despawn_distance());
            let no_despawn_distance_sq = no_despawn_distance * no_despawn_distance;
            let no_action_since = inner
                .natural_mob_no_action_since_tick
                .entry(entity_id)
                .or_insert(current_tick);
            if let Some(last_damage_tick) = snapshot.retained.last_damage_tick
                && last_damage_tick > *no_action_since
            {
                *no_action_since = last_damage_tick;
            }
            if nearest_distance_sq < no_despawn_distance_sq {
                *no_action_since = current_tick;
            }
            let no_action_time = current_tick.saturating_sub(*no_action_since);
            let should_soft_despawn = no_action_time > NATURAL_MOB_SOFT_DESPAWN_IDLE_TICKS
                && nearest_distance_sq > no_despawn_distance_sq
                && natural_mob_soft_despawn_roll(snapshot.uuid, current_tick);
            if !should_hard_despawn && !should_soft_despawn {
                continue;
            }
            let Some((_, mut entity_dispatches)) =
                remove_server_entity_locked(&mut inner, entity_id)
            else {
                continue;
            };
            #[cfg(test)]
            {
                removed += 1;
            }
            dispatches.append(&mut entity_dispatches);
        }
        drop(inner);
        NaturalMobDespawnOutcome {
            #[cfg(test)]
            removed,
            dispatches,
        }
    }

    pub(in crate::play) fn spawn_falling_block(
        &self,
        entity_type_id: i32,
        position: Vec3,
        block_state: BlockStateId,
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_session_entities("spawn falling block");
        spawn_falling_block_locked(&mut inner, entity_type_id, position, block_state)
    }

    pub(in crate::play) fn spawn_falling_block_owned(
        &self,
        _authority: &SimulationAuthority,
        entity_type_id: i32,
        position: Vec3,
        block_state: BlockStateId,
    ) -> Vec<VisibilityDispatch> {
        self.spawn_falling_block(entity_type_id, position, block_state)
    }

    pub(in crate::play) fn spawn_command_entity(
        &self,
        _authority: &SimulationAuthority,
        entity_type_id: i32,
        entity_type_name: String,
        position: Vec3,
    ) -> Vec<VisibilityDispatch> {
        self.spawn_command_entity_owned(entity_type_id, entity_type_name, position)
    }

    fn spawn_command_entity_owned(
        &self,
        entity_type_id: i32,
        entity_type_name: String,
        position: Vec3,
    ) -> Vec<VisibilityDispatch> {
        let chunk = chunk_pos_from_coords(position.x, position.z);
        let hostile = is_hostile_entity(&entity_type_name);
        let mob_behaviors = self.mob_behavior_table();
        let mut inner = self.lock_session_entities("spawn command entity");
        let active = hostile && inner.loaded_chunk_refcounts.contains_key(&chunk);
        let (entity_id, dispatches) = spawn_command_entity_locked(
            &mut inner,
            entity_type_id,
            entity_type_name,
            position,
            &mob_behaviors,
        );
        drop(inner);
        if active {
            self.publish_active_hostile_entity(entity_id);
        }
        dispatches
    }

    #[cfg(test)]
    pub(crate) fn spawn_script_router_test_entity(
        &self,
        entity_type_id: i32,
        entity_type_name: &str,
        position: Vec3,
    ) -> EntityId {
        let mob_behaviors = self.mob_behavior_table();
        let mut inner = self.lock_session_entities("spawn script router test entity");
        let (entity_id, _) = spawn_command_entity_locked(
            &mut inner,
            entity_type_id,
            entity_type_name.to_owned(),
            position,
            &mob_behaviors,
        );
        entity_id
    }

    pub(in crate::play) fn tick_dying_entities(
        &self,
        _authority: &SimulationAuthority,
        current_tick: u64,
    ) -> Vec<VisibilityDispatch> {
        self.synchronize_entity_lifecycle_epoch(current_tick);
        let mut inner = self.lock_session_entities("tick dying entities");
        let mut dispatches = finish_dying_entities_locked(&mut inner, current_tick);
        dispatches.extend(
            super::zombie_villager::finish_due_zombie_villager_conversions_locked(
                &mut inner,
                current_tick,
            ),
        );
        dispatches
    }
}

fn natural_mob_remove_when_far_away(type_name: &str) -> bool {
    !matches!(
        type_name,
        "minecraft:sheep" | "minecraft:pig" | "minecraft:chicken" | "minecraft:cow"
    )
}

fn natural_mob_despawn_seed_mix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

pub(super) fn natural_mob_soft_despawn_roll(uuid: uuid::Uuid, tick: u64) -> bool {
    let raw = uuid.as_u128();
    let seed = natural_mob_despawn_seed_mix(
        ((raw >> 64) as u64) ^ (raw as u64).rotate_left(23) ^ tick.rotate_left(41),
    );
    let mut state = (seed ^ JAVA_RANDOM_MULTIPLIER) & JAVA_RANDOM_MASK;
    loop {
        state = state
            .wrapping_mul(JAVA_RANDOM_MULTIPLIER)
            .wrapping_add(JAVA_RANDOM_ADDEND)
            & JAVA_RANDOM_MASK;
        let bits = (state >> 17) as u32;
        let value = bits % NATURAL_MOB_SOFT_DESPAWN_ROLL_BOUND;
        if bits
            .wrapping_sub(value)
            .wrapping_add(NATURAL_MOB_SOFT_DESPAWN_ROLL_BOUND - 1) as i32
            >= 0
        {
            return value == 0;
        }
    }
}

pub(super) fn spawn_falling_block_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_type_id: i32,
    position: Vec3,
    block_state: BlockStateId,
) -> Vec<VisibilityDispatch> {
    let mut entity = SpawnEntity::new(entity_type_id, "minecraft:falling_block", position);
    entity.block_state = Some(block_state.0);
    entity.on_ground = false;
    entity.retained.spawn_tick = inner.entity_lifecycle_tick;
    let id = inner.entities.spawn(entity);
    inner
        .entity_type_aabbs
        .entry(entity_type_id)
        .or_insert_with(|| entity_aabb("minecraft:falling_block"));
    track_entity_chunk_locked(inner, id, position);
    initialize_entity_wire_state_locked(inner, id);
    spawn_entity_visibility_locked(inner, id)
}

pub(super) fn spawn_command_entity_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_type_id: i32,
    entity_type_name: String,
    position: Vec3,
    mob_behaviors: &mc_data::mob_behavior_26_1_2::MobBehaviorTable,
) -> (EntityId, Vec<VisibilityDispatch>) {
    let hostile = is_hostile_entity(&entity_type_name);
    let mut entity = SpawnEntity::new(entity_type_id, entity_type_name, position);
    apply_entity_facts(&mut entity);
    apply_default_mob_goal(&mut entity, mob_behaviors);
    entity.retained.spawn_tick = inner.entity_lifecycle_tick;
    let aabb = entity_aabb(&entity.type_name);
    let is_sheep = entity.type_name == "minecraft:sheep";
    let animal = entity.animal;
    let id = inner.entities.spawn(entity);
    if hostile {
        inner.hostile_entities.insert(id);
    }
    if is_sheep {
        inner.sheep_entities.insert(id);
    }
    update_breeding_tick_tracking_locked(inner, id, animal);
    inner
        .entity_type_aabbs
        .entry(entity_type_id)
        .or_insert(aabb);
    track_entity_chunk_locked(inner, id, position);
    initialize_entity_wire_state_locked(inner, id);
    (id, spawn_entity_visibility_locked(inner, id))
}

pub(super) fn finish_dying_entities_locked(
    inner: &mut SessionEntityGuards<'_>,
    current_tick: u64,
) -> Vec<VisibilityDispatch> {
    let mut due_ids = Vec::with_capacity(DEATH_REMOVALS_PER_TICK);
    while due_ids.len() < DEATH_REMOVALS_PER_TICK {
        let Some((&deadline, _)) = inner.dying_entity_deadlines.first_key_value() else {
            break;
        };
        if deadline > current_tick {
            break;
        }
        let queue = inner
            .dying_entity_deadlines
            .get_mut(&deadline)
            .expect("first death deadline exists");
        let entity_id = queue
            .pop_front()
            .expect("death deadline queue is non-empty");
        let remove_deadline = queue.is_empty();
        if remove_deadline {
            inner.dying_entity_deadlines.remove(&deadline);
        }
        if inner.dying_entity_deadline_by_id.get(&entity_id) != Some(&deadline) {
            continue;
        }
        inner.dying_entity_deadline_by_id.remove(&entity_id);
        due_ids.push(entity_id);
    }

    let mut dispatches = Vec::new();
    for entity_id in due_ids {
        let Some(expected) = inner.entities.snapshot(entity_id) else {
            continue;
        };
        dispatches.extend(finish_one_dying_entity_locked(
            inner,
            current_tick,
            expected,
        ));
    }
    dispatches
}

pub(super) fn finish_one_dying_entity_locked(
    inner: &mut SessionEntityGuards<'_>,
    current_tick: u64,
    expected: EntitySnapshot,
) -> Vec<VisibilityDispatch> {
    let entity_id = expected.id;
    if expected.lifecycle != EntityLifecycle::Despawning
        || expected
            .retained
            .death_remove_tick
            .is_none_or(|remove_tick| remove_tick > current_tick)
    {
        schedule_entity_death_locked(inner, &expected);
        return Vec::new();
    }
    if let Some(removed) = inner.entities.remove_if_current(expected) {
        let snapshot = server_entity_snapshot_from(removed);
        let mut dispatches =
            entity_event_dispatches_locked(inner, entity_id, ENTITY_EVENT_DEATH_COMPLETE);
        clear_removed_entity_tracking_locked(inner, entity_id);
        dispatches.extend(despawn_entity_visibility_locked(inner, &snapshot));
        return dispatches;
    }
    if let Some(current) = inner.entities.snapshot(entity_id) {
        if current.lifecycle == EntityLifecycle::Despawning
            && current
                .retained
                .death_remove_tick
                .is_some_and(|deadline| deadline <= current_tick)
        {
            enqueue_entity_death_deadline_locked(inner, current_tick.saturating_add(1), entity_id);
        } else {
            schedule_entity_death_locked(inner, &current);
        }
    }
    Vec::new()
}

pub(super) fn schedule_entity_death_locked(
    inner: &mut SessionRegistryInner,
    entity: &EntitySnapshot,
) {
    if entity.lifecycle == EntityLifecycle::Despawning
        && let Some(remove_tick) = entity.retained.death_remove_tick
    {
        enqueue_entity_death_deadline_locked(inner, remove_tick, entity.id);
    }
}

fn enqueue_entity_death_deadline_locked(
    inner: &mut SessionRegistryInner,
    remove_tick: u64,
    entity_id: EntityId,
) {
    if inner.dying_entity_deadline_by_id.get(&entity_id) == Some(&remove_tick) {
        return;
    }
    inner
        .dying_entity_deadline_by_id
        .insert(entity_id, remove_tick);
    inner
        .dying_entity_deadlines
        .entry(remove_tick)
        .or_default()
        .push_back(entity_id);
}

#[cfg(test)]
pub(super) fn nearby_entity_snapshots_locked(
    inner: &SessionEntityGuards<'_>,
    position: Vec3,
    radius: f64,
    predicate: impl Fn(&EntitySnapshot) -> bool,
) -> Vec<ServerEntitySnapshot> {
    let radius_sq = radius * radius;
    nearby_entity_candidate_ids_locked(inner, position, radius)
        .into_iter()
        .filter_map(|id| inner.entities.snapshot(id))
        .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
        .filter(|entity| predicate(entity))
        .filter(|entity| distance_sq(entity.position, position) <= radius_sq)
        .map(server_entity_snapshot_from)
        .collect()
}

pub(super) fn nearby_entity_candidate_ids_locked(
    inner: &SessionRegistryInner,
    position: Vec3,
    radius: f64,
) -> Vec<EntityId> {
    let radius = radius.abs();
    let (min_cx, min_cz) = chunk_pos_from_coords(position.x - radius, position.z - radius);
    let (max_cx, max_cz) = chunk_pos_from_coords(position.x + radius, position.z + radius);
    let mut ids = Vec::new();
    for cz in min_cz..=max_cz {
        for cx in min_cx..=max_cx {
            if let Some(chunk_ids) = inner.simulation_inputs.entities_in_chunk((cx, cz)) {
                ids.extend(chunk_ids.iter().copied());
            }
        }
    }
    ids.sort_unstable();
    ids.dedup();
    ids
}

pub(super) fn clear_removed_entity_tracking_locked(
    inner: &mut SessionRegistryInner,
    entity_id: EntityId,
) {
    inner.item_despawn_deadline_by_id.remove(&entity_id);
    inner
        .zombie_villager_conversion_deadline_by_id
        .remove(&entity_id);
    if let Some(deadline) = inner.primed_tnt_deadline_by_id.remove(&entity_id) {
        let remove_bucket = inner
            .primed_tnt_deadlines
            .get_mut(&deadline)
            .is_some_and(|bucket| {
                bucket.remove(&entity_id);
                bucket.is_empty()
            });
        if remove_bucket {
            inner.primed_tnt_deadlines.remove(&deadline);
        }
    }
    clear_entity_publication_state_locked(inner, entity_id);
    inner.hostile_entities.remove(&entity_id);
    inner.natural_hostile_mobs.remove(&entity_id);
    inner.natural_ground_mobs.remove(&entity_id);
    inner.natural_aquatic_mobs.remove(&entity_id);
    inner.natural_mob_no_action_since_tick.remove(&entity_id);
    inner.sheep_entities.remove(&entity_id);
    update_breeding_tick_tracking_locked(inner, entity_id, None);
    inner.simulation_inputs.remove_terrain_pathing([entity_id]);
    untrack_entity_chunk_locked(inner, entity_id);
}

pub(super) fn update_breeding_tick_tracking_locked(
    inner: &mut SessionRegistryInner,
    entity_id: EntityId,
    animal: Option<mc_entity::AnimalBreedingState>,
) {
    inner
        .simulation_inputs
        .update_breeding_tick_entity(entity_id, animal);
}

pub(super) fn remove_server_entity_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_id: EntityId,
) -> Option<(ServerEntitySnapshot, Vec<VisibilityDispatch>)> {
    let snapshot = remove_server_entity_state_locked(inner, entity_id)?;
    let dispatches = despawn_entity_visibility_locked(inner, &snapshot);
    Some((snapshot, dispatches))
}

pub(super) fn remove_server_entity_state_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_id: EntityId,
) -> Option<ServerEntitySnapshot> {
    let expected = inner.entities.snapshot(entity_id)?;
    let snapshot = inner
        .entities
        .remove_if_current(expected)
        .map(server_entity_snapshot_from)?;
    clear_removed_entity_tracking_locked(inner, entity_id);
    Some(snapshot)
}

pub(super) fn track_entity_chunk_locked(
    inner: &mut SessionRegistryInner,
    entity_id: EntityId,
    position: Vec3,
) {
    let chunk = chunk_pos_from_coords(position.x, position.z);
    inner.simulation_inputs.track_entity(chunk, entity_id);
}

pub(super) fn move_entity_chunk_locked(
    inner: &mut SessionRegistryInner,
    entity_id: EntityId,
    old_chunk: (i32, i32),
    new_chunk: (i32, i32),
) {
    let routed_old_chunk = inner.simulation_inputs.move_entity(entity_id, new_chunk);
    debug_assert_eq!(routed_old_chunk, Some(old_chunk));
}

fn untrack_entity_chunk_locked(inner: &mut SessionRegistryInner, entity_id: EntityId) {
    inner.simulation_inputs.untrack_entity(entity_id);
}
