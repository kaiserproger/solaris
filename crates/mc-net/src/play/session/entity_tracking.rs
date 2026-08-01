use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use mc_entity::{EntityId, Rotation, Vec3};

use crate::lock_policy::lock_benign_mutex;

const TRACKER_SHARD_COUNT: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct LastSentEntityState {
    pub(super) position: Vec3,
    pub(super) velocity: Vec3,
    pub(super) rotation: Rotation,
    pub(super) on_ground: bool,
    pub(super) tracking_update_count: u64,
    pub(super) teleport_delay: u16,
}

#[derive(Debug)]
pub(super) struct EntityMovementTrackers {
    shards: [Mutex<HashMap<EntityId, LastSentEntityState>>; TRACKER_SHARD_COUNT],
}

impl Default for EntityMovementTrackers {
    fn default() -> Self {
        Self {
            shards: std::array::from_fn(|_| Mutex::new(HashMap::new())),
        }
    }
}

impl EntityMovementTrackers {
    fn shard_index(entity_id: EntityId) -> usize {
        (entity_id.0 as u32 as usize) & (TRACKER_SHARD_COUNT - 1)
    }

    fn lock_shard(
        &self,
        shard_index: usize,
    ) -> std::sync::MutexGuard<'_, HashMap<EntityId, LastSentEntityState>> {
        lock_benign_mutex(&self.shards[shard_index], "play.entity_movement_tracker")
    }

    pub(super) fn get(&self, entity_id: EntityId) -> Option<LastSentEntityState> {
        self.lock_shard(Self::shard_index(entity_id))
            .get(&entity_id)
            .copied()
    }

    pub(super) fn insert(&self, entity_id: EntityId, state: LastSentEntityState) {
        self.lock_shard(Self::shard_index(entity_id))
            .insert(entity_id, state);
    }

    pub(super) fn get_or_insert(
        &self,
        entity_id: EntityId,
        initial: LastSentEntityState,
    ) -> LastSentEntityState {
        *self
            .lock_shard(Self::shard_index(entity_id))
            .entry(entity_id)
            .or_insert(initial)
    }

    pub(super) fn remove(&self, entity_id: EntityId) {
        self.lock_shard(Self::shard_index(entity_id))
            .remove(&entity_id);
    }

    pub(super) fn update(
        &self,
        entity_id: EntityId,
        update: impl FnOnce(&mut LastSentEntityState),
    ) -> bool {
        let mut shard = self.lock_shard(Self::shard_index(entity_id));
        let Some(state) = shard.get_mut(&entity_id) else {
            return false;
        };
        update(state);
        true
    }

    pub(super) fn compare_exchange(
        &self,
        entity_id: EntityId,
        expected: LastSentEntityState,
        next: LastSentEntityState,
    ) -> bool {
        let mut shard = self.lock_shard(Self::shard_index(entity_id));
        let Some(current) = shard.get_mut(&entity_id) else {
            return false;
        };
        if *current != expected {
            return false;
        }
        *current = next;
        true
    }

    pub(super) fn compare_exchange_many(
        &self,
        updates: Vec<(EntityId, LastSentEntityState, LastSentEntityState)>,
    ) -> HashSet<EntityId> {
        let mut by_shard: [Vec<_>; TRACKER_SHARD_COUNT] = std::array::from_fn(|_| Vec::new());
        for update in updates {
            by_shard[Self::shard_index(update.0)].push(update);
        }

        let mut accepted = HashSet::new();
        for (shard_index, updates) in by_shard.into_iter().enumerate() {
            if updates.is_empty() {
                continue;
            }
            let mut shard = self.lock_shard(shard_index);
            for (entity_id, expected, next) in updates {
                let Some(current) = shard.get_mut(&entity_id) else {
                    continue;
                };
                if *current == expected {
                    *current = next;
                    accepted.insert(entity_id);
                }
            }
        }
        accepted
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.shards
            .iter()
            .enumerate()
            .all(|(index, _)| self.lock_shard(index).is_empty())
    }
}
