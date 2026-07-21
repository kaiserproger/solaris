use mc_entity::{EntityId, EntityLifecycle, EntitySnapshot, SpawnEntity, Vec3};
use mc_world::BlockStateId;
use std::sync::atomic::Ordering;

use crate::play::simulation::SimulationAuthority;
use crate::play::spawn::chunk_pos_from_coords;

#[cfg(test)]
use super::interaction_geometry::distance_sq;
use super::interaction_geometry::entity_aabb;
use super::outbound::{ServerEntitySnapshot, VisibilityDispatch};
use super::visibility::{
    clear_entity_publication_state_locked, despawn_entity_visibility_locked,
    entity_event_dispatches_locked, initialize_entity_wire_state_locked,
    server_entity_snapshot_from, spawn_entity_visibility_locked,
};
use super::{SessionEntityGuards, SessionRegistry, SessionRegistryInner, apply_entity_facts};

pub(super) const ENTITY_EVENT_DEATH_COMPLETE: i8 = 60;
pub(super) const DEATH_REMOVALS_PER_TICK: usize = 4;

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
        let mut inner = self.lock_session_entities("spawn command entity");
        spawn_command_entity_locked(&mut inner, entity_type_id, entity_type_name, position)
    }

    pub(in crate::play) fn tick_dying_entities(
        &self,
        _authority: &SimulationAuthority,
        current_tick: u64,
    ) -> Vec<VisibilityDispatch> {
        self.synchronize_entity_lifecycle_epoch(current_tick);
        let mut inner = self.lock_session_entities("tick dying entities");
        finish_dying_entities_locked(&mut inner, current_tick)
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
) -> Vec<VisibilityDispatch> {
    let mut entity = SpawnEntity::new(entity_type_id, entity_type_name, position);
    apply_entity_facts(&mut entity);
    entity.retained.spawn_tick = inner.entity_lifecycle_tick;
    let aabb = entity_aabb(&entity.type_name);
    let id = inner.entities.spawn(entity);
    inner
        .entity_type_aabbs
        .entry(entity_type_id)
        .or_insert(aabb);
    track_entity_chunk_locked(inner, id, position);
    initialize_entity_wire_state_locked(inner, id);
    spawn_entity_visibility_locked(inner, id)
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
            if let Some(chunk_ids) = inner.entities_by_chunk.get(&(cx, cz)) {
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
    clear_entity_publication_state_locked(inner, entity_id);
    inner.terrain_pathing_entities.remove(&entity_id);
    untrack_entity_chunk_locked(inner, entity_id);
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
    inner.entity_chunks.insert(entity_id, chunk);
    inner
        .entities_by_chunk
        .entry(chunk)
        .or_default()
        .insert(entity_id);
}

pub(super) fn move_entity_chunk_locked(
    inner: &mut SessionRegistryInner,
    entity_id: EntityId,
    old_chunk: (i32, i32),
    new_chunk: (i32, i32),
) {
    if let Some(entities) = inner.entities_by_chunk.get_mut(&old_chunk) {
        entities.remove(&entity_id);
        if entities.is_empty() {
            inner.entities_by_chunk.remove(&old_chunk);
        }
    }
    inner.entity_chunks.insert(entity_id, new_chunk);
    inner
        .entities_by_chunk
        .entry(new_chunk)
        .or_default()
        .insert(entity_id);
}

fn untrack_entity_chunk_locked(inner: &mut SessionRegistryInner, entity_id: EntityId) {
    if let Some(chunk) = inner.entity_chunks.remove(&entity_id)
        && let Some(entities) = inner.entities_by_chunk.get_mut(&chunk)
    {
        entities.remove(&entity_id);
        if entities.is_empty() {
            inner.entities_by_chunk.remove(&chunk);
        }
    }
}
