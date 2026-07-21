use std::collections::{HashMap, HashSet};
use std::hash::Hash;
use std::sync::Arc;

use mc_entity::{EntityId, EntityLifecycle, EntitySnapshot, Rotation, Vec3};
use mc_protocol::packets::play::MoveEntityPosRot;

use crate::play::PlayerPose;
use crate::play::wire_entities::ServerEntityWireMove;

use super::outbound::{
    OutboundCommand, PlayerEntitySnapshot, ServerEntityMove, ServerEntitySnapshot,
    SessionRecipient, VisibilityDispatch,
};
use super::{
    PlaySession, SessionEntityGuards, SessionId, SessionRegistryInner,
    record_entity_dispatches_locked,
};

pub(super) fn ordered_session_recipient(id: SessionId, session: &PlaySession) -> SessionRecipient {
    SessionRecipient::ordered(
        id,
        session.tx.clone(),
        Arc::clone(&session.pressure),
        &session.ordered_dispatch,
    )
}

fn ordered_spawn_session_recipient(id: SessionId, session: &PlaySession) -> SessionRecipient {
    SessionRecipient::ordered_spawn(
        id,
        session.tx.clone(),
        Arc::clone(&session.pressure),
        &session.ordered_dispatch,
    )
}

#[derive(Debug, Clone, Copy)]
pub(super) struct LastSentEntityState {
    pub(super) position: Vec3,
    pub(super) velocity: Vec3,
    pub(super) rotation: Rotation,
    pub(super) on_ground: bool,
    pub(super) tracking_update_count: u64,
    pub(super) teleport_delay: u16,
}

const ENTITY_POSITION_DIRTY_THRESHOLD_SQUARED: f64 = 7.629_394_531_25e-6;
const ENTITY_POSITION_REFRESH_UPDATES: u64 = 60;
const ENTITY_ABSOLUTE_REFRESH_UPDATES: u16 = 400;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum EntityPositionUpdate {
    None,
    Relative(Vec3),
    Absolute,
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct EntityTrackerUpdate {
    pub(super) wire_move: Option<ServerEntityWireMove>,
    pub(super) send_velocity: bool,
    pub(super) send_head_rotation: bool,
}

pub(super) fn quantized_entity_delta(current: Vec3, previous: Vec3) -> Vec3 {
    fn java_round(value: f64) -> f64 {
        (value + 0.5).floor()
    }

    fn axis(current: f64, previous: f64) -> f64 {
        (java_round(current * 4096.0) - java_round(previous * 4096.0)) / 4096.0
    }

    Vec3::new(
        axis(current.x, previous.x),
        axis(current.y, previous.y),
        axis(current.z, previous.z),
    )
}

fn relative_entity_delta_fits(delta: Vec3) -> bool {
    [delta.x, delta.y, delta.z].into_iter().all(|axis| {
        let encoded = axis * 4096.0;
        encoded >= f64::from(i16::MIN) && encoded <= f64::from(i16::MAX)
    })
}

fn entity_displacement_is_dirty(last_sent: &LastSentEntityState, position: Vec3) -> bool {
    let dx = position.x - last_sent.position.x;
    let dy = position.y - last_sent.position.y;
    let dz = position.z - last_sent.position.z;
    dx * dx + dy * dy + dz * dz >= ENTITY_POSITION_DIRTY_THRESHOLD_SQUARED
}

fn entity_position_is_dirty(last_sent: &LastSentEntityState, position: Vec3) -> bool {
    entity_displacement_is_dirty(last_sent, position)
        || last_sent
            .tracking_update_count
            .is_multiple_of(ENTITY_POSITION_REFRESH_UPDATES)
}

fn choose_entity_position_update(
    last_sent: &LastSentEntityState,
    position: Vec3,
    on_ground: bool,
    position_dirty: bool,
) -> EntityPositionUpdate {
    let delta = quantized_entity_delta(position, last_sent.position);
    if last_sent.on_ground != on_ground
        || last_sent.teleport_delay > ENTITY_ABSOLUTE_REFRESH_UPDATES
        || !relative_entity_delta_fits(delta)
    {
        return EntityPositionUpdate::Absolute;
    }
    if position_dirty {
        return EntityPositionUpdate::Relative(delta);
    }
    EntityPositionUpdate::None
}

pub(super) fn plan_entity_position_update(
    last_sent: &mut LastSentEntityState,
    position: Vec3,
    on_ground: bool,
) -> EntityPositionUpdate {
    last_sent.teleport_delay = last_sent.teleport_delay.saturating_add(1);
    let position_dirty = entity_position_is_dirty(last_sent, position);
    let update = choose_entity_position_update(last_sent, position, on_ground, position_dirty);

    match update {
        EntityPositionUpdate::Relative(_) => {
            last_sent.position = position;
            last_sent.on_ground = on_ground;
        }
        EntityPositionUpdate::Absolute => {
            last_sent.position = position;
            last_sent.on_ground = on_ground;
            last_sent.teleport_delay = 0;
        }
        EntityPositionUpdate::None => {}
    }
    last_sent.tracking_update_count = last_sent.tracking_update_count.wrapping_add(1);
    update
}

pub(super) fn entity_wire_move(
    position_update: EntityPositionUpdate,
    body_rotation_changed: bool,
    position: Vec3,
) -> Option<ServerEntityWireMove> {
    entity_wire_move_for_kind(position_update, body_rotation_changed, position, false)
}

pub(super) fn entity_wire_move_for_kind(
    position_update: EntityPositionUpdate,
    body_rotation_changed: bool,
    position: Vec3,
    is_arrow: bool,
) -> Option<ServerEntityWireMove> {
    match (position_update, body_rotation_changed, is_arrow) {
        (EntityPositionUpdate::Absolute, _, _) => Some(ServerEntityWireMove::Absolute { position }),
        (EntityPositionUpdate::Relative(delta), true, _)
        | (EntityPositionUpdate::Relative(delta), false, true) => {
            Some(ServerEntityWireMove::PositionRotation { delta })
        }
        (EntityPositionUpdate::Relative(delta), false, false) => {
            Some(ServerEntityWireMove::Position { delta })
        }
        (EntityPositionUpdate::None, true, true) => {
            Some(ServerEntityWireMove::PositionRotation { delta: Vec3::ZERO })
        }
        (EntityPositionUpdate::None, true, false) => Some(ServerEntityWireMove::Rotation),
        (EntityPositionUpdate::None, false, _) => None,
    }
}

#[cfg(test)]
pub(super) fn advance_entity_tracker_update(
    last_sent: &mut LastSentEntityState,
    position: Vec3,
    velocity: Vec3,
    rotation: Rotation,
    on_ground: bool,
    sends_velocity: bool,
) -> EntityTrackerUpdate {
    let body_rotation_changed = packed_rotation_changed(last_sent.rotation, rotation);
    let send_head_rotation = packed_head_yaw_changed(last_sent.rotation, rotation);
    let send_velocity = sends_velocity && entity_velocity_changed(last_sent.velocity, velocity);
    let position_update = plan_entity_position_update(last_sent, position, on_ground);
    let wire_move = entity_wire_move(position_update, body_rotation_changed, position);

    if matches!(
        wire_move,
        Some(
            ServerEntityWireMove::Rotation
                | ServerEntityWireMove::PositionRotation { .. }
                | ServerEntityWireMove::Absolute { .. }
        )
    ) {
        last_sent.rotation.yaw = rotation.yaw;
        last_sent.rotation.pitch = rotation.pitch;
        last_sent.on_ground = on_ground;
    }
    if send_head_rotation {
        last_sent.rotation.head_yaw = rotation.head_yaw;
    }
    if send_velocity {
        last_sent.velocity = velocity;
    }

    EntityTrackerUpdate {
        wire_move,
        send_velocity,
        send_head_rotation,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisibilityTransition {
    Spawn,
    Despawn,
}

struct EntityPublication {
    snapshot: ServerEntitySnapshot,
    recipients: Vec<SessionRecipient>,
}

pub(super) fn finish_player_pose_locked(
    inner: &mut SessionRegistryInner,
    id: SessionId,
    pose: PlayerPose,
    old_chunk: (i32, i32),
    old_observers: &HashSet<SessionId>,
) -> Vec<VisibilityDispatch> {
    if !inner.sessions.contains_key(&id) {
        return Vec::new();
    }
    let mut dispatches = Vec::new();
    let crossed_chunk = old_chunk != pose.chunk_pos();
    if crossed_chunk {
        dispatches.extend(refresh_player_target_visibility_locked(
            inner,
            id,
            old_chunk,
            pose.chunk_pos(),
        ));
    }
    let new_observers = visible_observers_locked(inner, id);
    let Some(snapshot) = inner
        .sessions
        .get(&id)
        .map(|session| session_snapshot(id, session))
    else {
        return dispatches;
    };
    let move_recipients =
        session_recipients(inner, old_observers.intersection(&new_observers).copied());
    dispatches.extend(visibility_dispatches(move_recipients, || {
        OutboundCommand::MovePlayer(snapshot.clone())
    }));
    dispatches
}

pub(super) fn session_snapshot(id: SessionId, session: &PlaySession) -> PlayerEntitySnapshot {
    PlayerEntitySnapshot {
        session_id: id,
        entity_id: session.entity_id,
        uuid: session.uuid,
        name: session.name.clone(),
        properties: session.properties.clone(),
        pose: session.pose,
    }
}

pub(in crate::play) fn server_entity_snapshot_from(entity: EntitySnapshot) -> ServerEntitySnapshot {
    let has_living_health = mc_data::entity_types::entity_type_contract_26_1_2_by_name(
        &entity.type_name,
    )
    .is_some_and(|contract| {
        contract.behavior.archetype == mc_data::entity_types::EntityArchetype::Living
    });
    ServerEntitySnapshot {
        id: entity.id,
        uuid: entity.uuid,
        type_id: entity.type_id,
        type_name: entity.type_name,
        position: entity.position,
        rotation: entity.rotation,
        velocity: entity.velocity,
        on_ground: entity.on_ground,
        health: has_living_health.then_some(entity.health),
        item_stack: entity.item_stack,
        experience_value: entity.experience_value,
        block_state: entity.block_state,
        animal: entity.animal,
    }
}

pub(super) fn spawned_xp_observer_ids(dispatches: &[VisibilityDispatch]) -> Vec<SessionId> {
    dispatches
        .iter()
        .filter_map(|dispatch| match &dispatch.command {
            OutboundCommand::SpawnEntity(entity)
                if entity.experience_value.is_some_and(|value| value > 0) =>
            {
                Some(dispatch.recipient.id)
            }
            _ => None,
        })
        .collect()
}

pub(super) fn publish_player_body_snapshot_locked(
    inner: &mut SessionRegistryInner,
    pushed: ServerEntitySnapshot,
) -> ServerEntitySnapshot {
    if let Some(snapshot) = inner.published_entity_snapshots.get_mut(&pushed.id) {
        snapshot.position = pushed.position;
        snapshot.clone()
    } else {
        inner
            .published_entity_snapshots
            .insert(pushed.id, pushed.clone());
        pushed
    }
}

pub(super) fn publish_entity_movement_locked(
    inner: &mut SessionRegistryInner,
    snapshot: &ServerEntitySnapshot,
    old_observers: &HashSet<SessionId>,
    new_observers: &HashSet<SessionId>,
) -> Vec<VisibilityDispatch> {
    let entity_id = snapshot.id;
    let position = snapshot.position;
    let Some(last_sent) = inner.last_sent_entity_states.get(&entity_id).copied() else {
        initialize_entity_wire_state_from_snapshot_locked(inner, snapshot);
        return Vec::new();
    };
    let position_update = choose_entity_position_update(
        &last_sent,
        position,
        snapshot.on_ground,
        entity_displacement_is_dirty(&last_sent, position),
    );
    if position_update == EntityPositionUpdate::None {
        return Vec::new();
    }
    if let Some(sent) = inner.last_sent_entity_states.get_mut(&entity_id) {
        sent.position = position;
        sent.on_ground = snapshot.on_ground;
        if position_update == EntityPositionUpdate::Absolute {
            sent.rotation.yaw = snapshot.rotation.yaw;
            sent.rotation.pitch = snapshot.rotation.pitch;
            sent.teleport_delay = 0;
        }
    }
    let wire_move = entity_wire_move(position_update, false, position);
    let dispatches = old_observers
        .intersection(new_observers)
        .filter_map(|observer_id| {
            let observer = inner.sessions.get(observer_id)?;
            Some(VisibilityDispatch {
                recipient: ordered_session_recipient(*observer_id, observer),
                command: OutboundCommand::MoveEntityRelative(ServerEntityMove {
                    id: entity_id,
                    wire_move,
                    velocity: snapshot.velocity,
                    rotation: snapshot.rotation,
                    on_ground: snapshot.on_ground,
                    send_velocity: false,
                    send_head_rotation: false,
                }),
            })
        })
        .collect::<Vec<_>>();
    record_entity_dispatches_locked(inner, &dispatches);
    dispatches
}

pub(super) fn visible_observers_locked(
    inner: &SessionRegistryInner,
    target: SessionId,
) -> HashSet<SessionId> {
    inner
        .sessions
        .iter()
        .filter_map(|(&observer_id, observer)| {
            observer
                .visible_players
                .contains(&target)
                .then_some(observer_id)
        })
        .collect()
}

pub(super) fn session_recipients<I>(inner: &SessionRegistryInner, ids: I) -> Vec<SessionRecipient>
where
    I: IntoIterator<Item = SessionId>,
{
    ids.into_iter()
        .filter_map(|id| {
            inner
                .sessions
                .get(&id)
                .map(|session| ordered_session_recipient(id, session))
        })
        .collect()
}

pub(super) fn remove_player_visibility_locked(
    inner: &mut SessionRegistryInner,
    target: SessionId,
) -> Vec<SessionRecipient> {
    inner
        .sessions
        .iter_mut()
        .filter_map(|(&observer_id, observer)| {
            observer
                .visible_players
                .remove(&target)
                .then(|| ordered_session_recipient(observer_id, observer))
        })
        .collect()
}

pub(super) fn visibility_dispatches(
    recipients: Vec<SessionRecipient>,
    mut command: impl FnMut() -> OutboundCommand,
) -> Vec<VisibilityDispatch> {
    recipients
        .into_iter()
        .map(|recipient| VisibilityDispatch {
            recipient,
            command: command(),
        })
        .collect()
}

pub(super) fn refresh_loaded_chunk_for_session_locked(
    inner: &mut SessionRegistryInner,
    observer_id: SessionId,
    chunk: (i32, i32),
) -> Vec<VisibilityDispatch> {
    if !inner.sessions.contains_key(&observer_id) {
        return Vec::new();
    }
    let players = inner
        .sessions
        .iter()
        .filter(|&(&target_id, target)| {
            target_id != observer_id && target.pose.chunk_pos() == chunk
        })
        .map(|(&target_id, target)| (target_id, session_snapshot(target_id, target)))
        .collect::<Vec<_>>();
    let entities = inner
        .entities_by_chunk
        .get(&chunk)
        .into_iter()
        .flat_map(|entities| entities.iter().copied())
        .filter_map(|entity_id| inner.published_entity_snapshots.get(&entity_id).cloned())
        .collect::<Vec<_>>();

    let Some(observer) = inner.sessions.get_mut(&observer_id) else {
        return Vec::new();
    };
    let mut dispatches = Vec::new();
    for (target_id, snapshot) in players {
        if observer.visible_players.insert(target_id) {
            dispatches.push(VisibilityDispatch {
                recipient: ordered_spawn_session_recipient(observer_id, observer),
                command: OutboundCommand::SpawnPlayer(snapshot),
            });
        }
    }
    for snapshot in entities {
        if Arc::make_mut(&mut observer.visible_entities).insert(snapshot.id) {
            dispatches.push(VisibilityDispatch {
                recipient: ordered_spawn_session_recipient(observer_id, observer),
                command: OutboundCommand::SpawnEntity(snapshot),
            });
        }
    }
    record_entity_dispatches_locked(inner, &dispatches);
    dispatches
}

pub(super) fn refresh_unloaded_chunk_for_session_locked(
    inner: &mut SessionRegistryInner,
    observer_id: SessionId,
    chunk: (i32, i32),
) -> Vec<VisibilityDispatch> {
    let Some(observer) = inner.sessions.get(&observer_id) else {
        return Vec::new();
    };
    let visible_players = observer.visible_players.clone();
    let visible_entities = observer.visible_entities.clone();

    let players = visible_players
        .into_iter()
        .filter_map(|target_id| {
            let target = inner.sessions.get(&target_id)?;
            (target.pose.chunk_pos() == chunk)
                .then(|| (target_id, session_snapshot(target_id, target)))
        })
        .collect::<Vec<_>>();
    let entities = visible_entities
        .iter()
        .copied()
        .filter_map(|entity_id| {
            (inner.entity_chunks.get(&entity_id).copied() == Some(chunk))
                .then(|| inner.published_entity_snapshots.get(&entity_id).cloned())?
        })
        .collect::<Vec<_>>();

    let Some(observer) = inner.sessions.get_mut(&observer_id) else {
        return Vec::new();
    };
    let mut dispatches = Vec::new();
    for (target_id, snapshot) in players {
        if observer.visible_players.remove(&target_id) {
            dispatches.push(VisibilityDispatch {
                recipient: ordered_session_recipient(observer_id, observer),
                command: OutboundCommand::DespawnPlayer(snapshot),
            });
        }
    }
    for snapshot in entities {
        if Arc::make_mut(&mut observer.visible_entities).remove(&snapshot.id) {
            dispatches.push(VisibilityDispatch {
                recipient: ordered_session_recipient(observer_id, observer),
                command: OutboundCommand::DespawnEntity(snapshot),
            });
        }
    }
    record_entity_dispatches_locked(inner, &dispatches);
    dispatches
}

pub(super) fn refresh_player_target_visibility_locked(
    inner: &mut SessionRegistryInner,
    target_id: SessionId,
    old_chunk: (i32, i32),
    new_chunk: (i32, i32),
) -> Vec<VisibilityDispatch> {
    let Some(snapshot) = inner
        .sessions
        .get(&target_id)
        .map(|session| session_snapshot(target_id, session))
    else {
        return Vec::new();
    };
    let mut observer_ids = visible_observers_locked(inner, target_id);
    for (&observer_id, observer) in &inner.sessions {
        if observer.loaded.contains(&old_chunk) || observer.loaded.contains(&new_chunk) {
            observer_ids.insert(observer_id);
        }
    }

    let mut dispatches = Vec::new();
    for observer_id in observer_ids {
        if observer_id == target_id {
            continue;
        }
        let Some(observer) = inner.sessions.get_mut(&observer_id) else {
            continue;
        };
        let desired = observer.loaded.contains(&new_chunk);
        match update_visibility_set(&mut observer.visible_players, target_id, desired) {
            Some(VisibilityTransition::Spawn) => {
                dispatches.push(VisibilityDispatch {
                    recipient: ordered_spawn_session_recipient(observer_id, observer),
                    command: OutboundCommand::SpawnPlayer(snapshot.clone()),
                });
            }
            Some(VisibilityTransition::Despawn) => {
                dispatches.push(VisibilityDispatch {
                    recipient: ordered_session_recipient(observer_id, observer),
                    command: OutboundCommand::DespawnPlayer(snapshot.clone()),
                });
            }
            None => {}
        }
    }
    dispatches
}

fn update_visibility_set<T>(
    visible: &mut HashSet<T>,
    target: T,
    desired: bool,
) -> Option<VisibilityTransition>
where
    T: Copy + Eq + Hash,
{
    if desired {
        visible
            .insert(target)
            .then_some(VisibilityTransition::Spawn)
    } else {
        visible
            .remove(&target)
            .then_some(VisibilityTransition::Despawn)
    }
}

pub(super) fn publish_server_entity_snapshot_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_id: EntityId,
) -> Option<ServerEntitySnapshot> {
    let snapshot = inner
        .entities
        .snapshot(entity_id)
        .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
        .map(server_entity_snapshot_from)?;
    inner
        .published_entity_snapshots
        .insert(entity_id, snapshot.clone());
    Some(snapshot)
}

pub(super) fn initialize_entity_wire_state_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_id: EntityId,
) {
    let Some(entity) = inner.entities.snapshot(entity_id) else {
        return;
    };
    inner.last_sent_entity_states.insert(
        entity_id,
        LastSentEntityState {
            position: entity.position,
            velocity: entity.velocity,
            rotation: entity.rotation,
            on_ground: entity.on_ground,
            tracking_update_count: 0,
            teleport_delay: 0,
        },
    );
}

pub(super) fn initialize_entity_wire_state_from_snapshot_locked(
    inner: &mut SessionRegistryInner,
    entity: &ServerEntitySnapshot,
) {
    inner.last_sent_entity_states.insert(
        entity.id,
        LastSentEntityState {
            position: entity.position,
            velocity: entity.velocity,
            rotation: entity.rotation,
            on_ground: entity.on_ground,
            tracking_update_count: 0,
            teleport_delay: 0,
        },
    );
}

pub(super) fn install_committed_entity_publications_locked(
    inner: &mut SessionRegistryInner,
    snapshots: Vec<ServerEntitySnapshot>,
) -> Vec<VisibilityDispatch> {
    let mut publications = Vec::with_capacity(snapshots.len());
    let mut publications_by_chunk = HashMap::<(i32, i32), Vec<usize>>::new();
    for snapshot in snapshots {
        inner
            .published_entity_snapshots
            .insert(snapshot.id, snapshot.clone());
        let chunk = inner.entity_chunks[&snapshot.id];
        let publication_index = publications.len();
        publications.push(EntityPublication {
            snapshot,
            recipients: Vec::new(),
        });
        publications_by_chunk
            .entry(chunk)
            .or_default()
            .push(publication_index);
    }

    for (&observer_id, observer) in &mut inner.sessions {
        for (chunk, publication_indexes) in &publications_by_chunk {
            if !observer.loaded.contains(chunk) {
                continue;
            }
            for &publication_index in publication_indexes {
                let publication = &mut publications[publication_index];
                if Arc::make_mut(&mut observer.visible_entities).insert(publication.snapshot.id) {
                    publication
                        .recipients
                        .push(ordered_spawn_session_recipient(observer_id, observer));
                }
            }
        }
    }

    let mut dispatches = Vec::new();
    for publication in publications {
        for recipient in publication.recipients {
            dispatches.push(VisibilityDispatch {
                recipient,
                command: OutboundCommand::SpawnEntity(publication.snapshot.clone()),
            });
        }
    }
    record_entity_dispatches_locked(inner, &dispatches);
    dispatches
}

pub(super) fn packed_rotation_changed(previous: Rotation, current: Rotation) -> bool {
    MoveEntityPosRot::pack_degrees(previous.yaw) != MoveEntityPosRot::pack_degrees(current.yaw)
        || MoveEntityPosRot::pack_degrees(previous.pitch)
            != MoveEntityPosRot::pack_degrees(current.pitch)
}

pub(super) fn packed_head_yaw_changed(previous: Rotation, current: Rotation) -> bool {
    MoveEntityPosRot::pack_degrees(previous.head_yaw)
        != MoveEntityPosRot::pack_degrees(current.head_yaw)
}

pub(super) fn entity_velocity_changed(previous: Vec3, current: Vec3) -> bool {
    let dx = current.x - previous.x;
    let dy = current.y - previous.y;
    let dz = current.z - previous.z;
    let difference_squared = dx * dx + dy * dy + dz * dz;
    difference_squared > 1.0e-7 || (difference_squared > 0.0 && current == Vec3::ZERO)
}

pub(super) fn spawn_entity_visibility_locked(
    inner: &mut SessionEntityGuards<'_>,
    entity_id: EntityId,
) -> Vec<VisibilityDispatch> {
    let Some(snapshot) = inner
        .entities
        .snapshot(entity_id)
        .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
        .map(server_entity_snapshot_from)
    else {
        return Vec::new();
    };
    spawn_entity_visibility_from_snapshot_locked(inner, snapshot)
}

pub(super) fn spawn_entity_visibility_from_snapshot_locked(
    inner: &mut SessionRegistryInner,
    snapshot: ServerEntitySnapshot,
) -> Vec<VisibilityDispatch> {
    let entity_id = snapshot.id;
    let Some(chunk) = inner.entity_chunks.get(&entity_id).copied() else {
        return Vec::new();
    };
    inner
        .published_entity_snapshots
        .insert(entity_id, snapshot.clone());
    let mut dispatches = Vec::new();
    for (&observer_id, observer) in &mut inner.sessions {
        if observer.loaded.contains(&chunk)
            && Arc::make_mut(&mut observer.visible_entities).insert(entity_id)
        {
            dispatches.push(VisibilityDispatch {
                recipient: ordered_spawn_session_recipient(observer_id, observer),
                command: OutboundCommand::SpawnEntity(snapshot.clone()),
            });
        }
    }
    record_entity_dispatches_locked(inner, &dispatches);
    dispatches
}

pub(super) fn refresh_entity_target_visibility_locked(
    inner: &mut SessionRegistryInner,
    entity_id: EntityId,
    old_chunk: (i32, i32),
    new_chunk: (i32, i32),
) -> Vec<VisibilityDispatch> {
    let Some(snapshot) = inner.published_entity_snapshots.get(&entity_id).cloned() else {
        return Vec::new();
    };
    let mut observer_ids = visible_entity_observers_locked(inner, entity_id);
    for (&observer_id, observer) in &inner.sessions {
        if observer.loaded.contains(&old_chunk) || observer.loaded.contains(&new_chunk) {
            observer_ids.insert(observer_id);
        }
    }

    let mut dispatches = Vec::new();
    for observer_id in observer_ids {
        let Some(observer) = inner.sessions.get_mut(&observer_id) else {
            continue;
        };
        let desired = observer.loaded.contains(&new_chunk);
        match update_visibility_set(
            Arc::make_mut(&mut observer.visible_entities),
            entity_id,
            desired,
        ) {
            Some(VisibilityTransition::Spawn) => {
                dispatches.push(VisibilityDispatch {
                    recipient: ordered_spawn_session_recipient(observer_id, observer),
                    command: OutboundCommand::SpawnEntity(snapshot.clone()),
                });
            }
            Some(VisibilityTransition::Despawn) => {
                dispatches.push(VisibilityDispatch {
                    recipient: ordered_session_recipient(observer_id, observer),
                    command: OutboundCommand::DespawnEntity(snapshot.clone()),
                });
            }
            None => {}
        }
    }
    record_entity_dispatches_locked(inner, &dispatches);
    dispatches
}

pub(super) fn visible_entity_observers_locked(
    inner: &SessionRegistryInner,
    target: EntityId,
) -> HashSet<SessionId> {
    inner
        .sessions
        .iter()
        .filter_map(|(&observer_id, observer)| {
            observer
                .visible_entities
                .contains(&target)
                .then_some(observer_id)
        })
        .collect()
}

pub(super) fn refresh_visibility_locked(
    inner: &mut SessionRegistryInner,
) -> Vec<VisibilityDispatch> {
    let ids: Vec<_> = inner.sessions.keys().copied().collect();
    let snapshots: HashMap<_, _> = inner
        .sessions
        .iter()
        .map(|(&id, session)| (id, session_snapshot(id, session)))
        .collect();
    let desired_by_observer: HashMap<_, HashSet<_>> = ids
        .iter()
        .filter_map(|observer_id| {
            let observer = inner.sessions.get(observer_id)?;
            let desired = ids
                .iter()
                .copied()
                .filter(|target_id| {
                    target_id != observer_id
                        && snapshots.get(target_id).is_some_and(|target| {
                            observer.loaded.contains(&target.pose.chunk_pos())
                        })
                })
                .collect();
            Some((*observer_id, desired))
        })
        .collect();
    let desired_entities_by_observer: HashMap<_, HashSet<_>> = ids
        .iter()
        .filter_map(|observer_id| {
            let observer = inner.sessions.get(observer_id)?;
            let desired = observer
                .loaded
                .iter()
                .filter_map(|chunk| inner.entities_by_chunk.get(chunk))
                .flat_map(|entities| entities.iter().copied())
                .filter(|entity_id| inner.published_entity_snapshots.contains_key(entity_id))
                .collect();
            Some((*observer_id, desired))
        })
        .collect();

    let mut dispatches = Vec::new();
    for observer_id in ids {
        let Some(observer) = inner.sessions.get_mut(&observer_id) else {
            continue;
        };
        let desired = desired_by_observer
            .get(&observer_id)
            .cloned()
            .unwrap_or_default();
        for target_id in desired.difference(&observer.visible_players) {
            if let Some(snapshot) = snapshots.get(target_id) {
                dispatches.push(VisibilityDispatch {
                    recipient: ordered_spawn_session_recipient(observer_id, observer),
                    command: OutboundCommand::SpawnPlayer(snapshot.clone()),
                });
            }
        }
        for target_id in observer.visible_players.difference(&desired) {
            if let Some(snapshot) = snapshots.get(target_id) {
                dispatches.push(VisibilityDispatch {
                    recipient: ordered_session_recipient(observer_id, observer),
                    command: OutboundCommand::DespawnPlayer(snapshot.clone()),
                });
            }
        }
        observer.visible_players = desired;

        let desired_entities = desired_entities_by_observer
            .get(&observer_id)
            .cloned()
            .unwrap_or_default();
        for entity_id in desired_entities.difference(&observer.visible_entities) {
            if let Some(snapshot) = inner.published_entity_snapshots.get(entity_id) {
                dispatches.push(VisibilityDispatch {
                    recipient: ordered_spawn_session_recipient(observer_id, observer),
                    command: OutboundCommand::SpawnEntity(snapshot.clone()),
                });
            }
        }
        for entity_id in observer.visible_entities.difference(&desired_entities) {
            if let Some(snapshot) = inner.published_entity_snapshots.get(entity_id) {
                dispatches.push(VisibilityDispatch {
                    recipient: ordered_session_recipient(observer_id, observer),
                    command: OutboundCommand::DespawnEntity(snapshot.clone()),
                });
            }
        }
        observer.visible_entities = Arc::new(desired_entities);
    }
    record_entity_dispatches_locked(inner, &dispatches);
    dispatches
}

pub(super) fn entity_event_dispatches_locked(
    inner: &SessionRegistryInner,
    entity_id: EntityId,
    event_id: i8,
) -> Vec<VisibilityDispatch> {
    visible_entity_observers_locked(inner, entity_id)
        .into_iter()
        .filter_map(|observer_id| {
            let observer = inner.sessions.get(&observer_id)?;
            Some(VisibilityDispatch {
                recipient: ordered_session_recipient(observer_id, observer),
                command: OutboundCommand::EntityEvent {
                    entity_id: entity_id.0,
                    event_id,
                },
            })
        })
        .collect()
}

pub(super) fn clear_entity_publication_state_locked(
    inner: &mut SessionRegistryInner,
    entity_id: EntityId,
) {
    inner.published_entity_snapshots.remove(&entity_id);
    inner.last_sent_entity_states.remove(&entity_id);
}

pub(super) fn despawn_entity_visibility_locked(
    inner: &mut SessionRegistryInner,
    snapshot: &ServerEntitySnapshot,
) -> Vec<VisibilityDispatch> {
    let observer_ids = remove_entity_visibility_locked(inner, snapshot.id);
    let dispatches = observer_ids
        .into_iter()
        .filter_map(|observer_id| {
            let observer = inner.sessions.get(&observer_id)?;
            Some(VisibilityDispatch {
                recipient: ordered_session_recipient(observer_id, observer),
                command: OutboundCommand::DespawnEntity(snapshot.clone()),
            })
        })
        .collect::<Vec<_>>();
    record_entity_dispatches_locked(inner, &dispatches);
    dispatches
}

pub(super) fn remove_entity_visibility_locked(
    inner: &mut SessionRegistryInner,
    entity_id: EntityId,
) -> Vec<SessionId> {
    inner
        .sessions
        .iter_mut()
        .filter_map(|(&observer_id, observer)| {
            Arc::make_mut(&mut observer.visible_entities)
                .remove(&entity_id)
                .then_some(observer_id)
        })
        .collect()
}
