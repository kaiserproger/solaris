use std::collections::{HashMap, HashSet};

use mc_entity::{EntityId, EntityLifecycle, EntitySnapshot, Vec3};

use crate::play::PlayerPose;

use super::entity_lifecycle::{
    move_entity_chunk_locked, nearby_entity_candidate_ids_locked, track_entity_chunk_locked,
};
use super::interaction_geometry::entity_geometry;
use super::outbound::{ServerEntitySnapshot, VisibilityDispatch};
use super::visibility::{
    finish_player_pose_locked, publish_entity_movement_locked, publish_player_body_snapshot_locked,
    refresh_entity_target_visibility_locked, spawn_entity_visibility_from_snapshot_locked,
    visible_entity_observers_locked, visible_observers_locked,
};
use super::{EntityStoreGuard, SessionId, SessionRegistryInner, chunk_pos_from_coords};

const PLAYER_BODY_HALF_WIDTH: f64 = 0.3;

pub(super) struct AcceptedPlayerPose {
    old_observers: HashSet<SessionId>,
    old_chunk: (i32, i32),
    old_prewarm_frontier: ((i32, i32), i32, f32),
    body_candidate_ids: HashSet<EntityId>,
}

impl AcceptedPlayerPose {
    pub(super) fn old_prewarm_frontier(&self) -> ((i32, i32), i32, f32) {
        self.old_prewarm_frontier
    }

    pub(super) fn take_body_candidate_ids(&mut self) -> HashSet<EntityId> {
        std::mem::take(&mut self.body_candidate_ids)
    }
}

pub(super) struct PlayerBodyPushes {
    expected_snapshots: Vec<EntitySnapshot>,
    #[cfg(test)]
    visited_entities: u64,
}

impl PlayerBodyPushes {
    pub(super) fn into_expected_snapshots(self) -> Vec<EntitySnapshot> {
        self.expected_snapshots
    }

    #[cfg(test)]
    pub(super) fn visited_entities(&self) -> u64 {
        self.visited_entities
    }
}

pub(super) fn accept_player_pose_locked(
    inner: &mut SessionRegistryInner,
    id: SessionId,
    pose: PlayerPose,
) -> Option<AcceptedPlayerPose> {
    let old_observers = visible_observers_locked(inner, id);
    let old_session = inner.sessions.get(&id)?;
    let old_chunk = old_session.pose.chunk_pos();
    let old_prewarm_frontier = (
        old_session.center,
        old_session.view_distance,
        old_session.pose.yaw,
    );
    inner.sessions.get_mut(&id)?.pose = pose;
    let max_entity_half_width = inner
        .entity_type_aabbs
        .values()
        .map(|aabb| aabb.half_width)
        .fold(mc_physics::Aabb::COW.half_width, f64::max);
    let player = Vec3::new(pose.x, pose.y, pose.z);
    let body_candidate_ids = nearby_entity_candidate_ids_locked(
        inner,
        player,
        PLAYER_BODY_HALF_WIDTH + max_entity_half_width,
    )
    .into_iter()
    .collect();
    Some(AcceptedPlayerPose {
        old_observers,
        old_chunk,
        old_prewarm_frontier,
        body_candidate_ids,
    })
}

pub(super) fn push_entities_from_player_locked(
    entities: &mut EntityStoreGuard<'_>,
    pose: PlayerPose,
    candidate_ids: &HashSet<EntityId>,
) -> PlayerBodyPushes {
    let player = Vec3::new(pose.x, pose.y, pose.z);
    let mut candidates = Vec::new();
    #[cfg(test)]
    let mut visited_entities = 0_u64;
    entities.visit_simulation_entities_for_ids(candidate_ids, |entity| {
        #[cfg(test)]
        {
            visited_entities += 1;
        }
        if entity.lifecycle == EntityLifecycle::Alive
            && mc_data::entity_types::fallback_entity_category(entity.type_name).is_living()
        {
            let aabb = entity_geometry(entity.type_name, entity.animal).aabb;
            candidates.push((entity.id, entity.position, aabb));
        }
    });

    let mut expected_snapshots = Vec::new();
    for (entity_id, entity_position, aabb) in candidates {
        let min_distance = PLAYER_BODY_HALF_WIDTH + aabb.half_width;
        let dx = entity_position.x - player.x;
        let dz = entity_position.z - player.z;
        let distance = dx.hypot(dz);
        if distance >= min_distance || (entity_position.y - player.y).abs() > 1.5 {
            continue;
        }
        let (nx, nz) = if distance > 1.0e-6 {
            (dx / distance, dz / distance)
        } else {
            let yaw = f64::from(pose.yaw).to_radians();
            (yaw.sin(), -yaw.cos())
        };
        let push = min_distance - distance + 0.02;
        let position = Vec3::new(
            entity_position.x + nx * push,
            entity_position.y,
            entity_position.z + nz * push,
        );
        if entities.set_position(entity_id, position)
            && let Some(entity) = entities
                .snapshot(entity_id)
                .filter(|entity| entity.lifecycle == EntityLifecycle::Alive)
        {
            expected_snapshots.push(entity);
        }
    }

    PlayerBodyPushes {
        expected_snapshots,
        #[cfg(test)]
        visited_entities,
    }
}

pub(super) fn filter_current_expected_entity_snapshots(
    expected: Vec<EntitySnapshot>,
    current: Vec<EntitySnapshot>,
) -> Vec<EntitySnapshot> {
    let current = current
        .into_iter()
        .map(|snapshot| (snapshot.id, snapshot))
        .collect::<HashMap<_, _>>();
    expected
        .into_iter()
        .filter(|snapshot| current.get(&snapshot.id) == Some(snapshot))
        .collect()
}

pub(super) fn publish_player_body_pushes_locked(
    inner: &mut SessionRegistryInner,
    pushed_entities: Vec<ServerEntitySnapshot>,
) -> Vec<VisibilityDispatch> {
    let mut dispatches = Vec::new();
    for pushed in pushed_entities {
        let entity_id = pushed.id;
        let position = pushed.position;
        let old_observers = visible_entity_observers_locked(inner, entity_id);
        let snapshot = publish_player_body_snapshot_locked(inner, pushed);
        let new_chunk = chunk_pos_from_coords(position.x, position.z);
        match inner.entity_chunks.get(&entity_id).copied() {
            Some(old_chunk) if old_chunk != new_chunk => {
                move_entity_chunk_locked(inner, entity_id, old_chunk, new_chunk);
                dispatches.extend(refresh_entity_target_visibility_locked(
                    inner, entity_id, old_chunk, new_chunk,
                ));
            }
            None => {
                track_entity_chunk_locked(inner, entity_id, position);
                dispatches.extend(spawn_entity_visibility_from_snapshot_locked(
                    inner,
                    snapshot.clone(),
                ));
            }
            Some(_) => {}
        }

        let new_observers = visible_entity_observers_locked(inner, entity_id);
        dispatches.extend(publish_entity_movement_locked(
            inner,
            &snapshot,
            &old_observers,
            &new_observers,
        ));
    }
    dispatches
}

pub(super) fn complete_accepted_player_pose_locked(
    inner: &mut SessionRegistryInner,
    id: SessionId,
    pose: PlayerPose,
    accepted: AcceptedPlayerPose,
    mut body_push_dispatches: Vec<VisibilityDispatch>,
) -> Vec<VisibilityDispatch> {
    body_push_dispatches.extend(finish_player_pose_locked(
        inner,
        id,
        pose,
        accepted.old_chunk,
        &accepted.old_observers,
    ));
    body_push_dispatches
}
