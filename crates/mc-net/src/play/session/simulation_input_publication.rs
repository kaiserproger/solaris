use super::*;

const ENTITY_INDEX_SHARDS: usize = 64;
type ChunkEntityIndex = HashMap<(i32, i32), Arc<HashSet<EntityId>>>;
type EntityChunkIndex = HashMap<EntityId, (i32, i32)>;

#[derive(Debug)]
pub(super) struct SimulationInputPublication {
    routing_revision: std::sync::atomic::AtomicU64,
    active_chunks: arc_swap::ArcSwap<HashSet<(i32, i32)>>,
    entity_chunks: [arc_swap::ArcSwap<ChunkEntityIndex>; ENTITY_INDEX_SHARDS],
    chunks_by_entity: [arc_swap::ArcSwap<EntityChunkIndex>; ENTITY_INDEX_SHARDS],
    terrain_pathing_entities: arc_swap::ArcSwap<HashSet<EntityId>>,
}

impl Default for SimulationInputPublication {
    fn default() -> Self {
        Self {
            routing_revision: std::sync::atomic::AtomicU64::new(0),
            active_chunks: arc_swap::ArcSwap::from_pointee(HashSet::new()),
            entity_chunks: std::array::from_fn(|_| arc_swap::ArcSwap::from_pointee(HashMap::new())),
            chunks_by_entity: std::array::from_fn(|_| {
                arc_swap::ArcSwap::from_pointee(HashMap::new())
            }),
            terrain_pathing_entities: arc_swap::ArcSwap::from_pointee(HashSet::new()),
        }
    }
}

impl SimulationInputPublication {
    pub(super) fn insert_active_chunk(&self, chunk: (i32, i32)) {
        let revision = self.begin_routing_update();
        let current = self.active_chunks.load_full();
        if current.contains(&chunk) {
            self.finish_routing_update(revision);
            return;
        }
        let mut next = (*current).clone();
        next.insert(chunk);
        self.active_chunks.store(Arc::new(next));
        self.finish_routing_update(revision);
    }

    pub(super) fn remove_active_chunk(&self, chunk: (i32, i32)) {
        let revision = self.begin_routing_update();
        let current = self.active_chunks.load_full();
        if !current.contains(&chunk) {
            self.finish_routing_update(revision);
            return;
        }
        let mut next = (*current).clone();
        next.remove(&chunk);
        self.active_chunks.store(Arc::new(next));
        self.finish_routing_update(revision);
    }

    pub(super) fn active_chunks(&self) -> Arc<HashSet<(i32, i32)>> {
        self.active_chunks.load_full()
    }

    pub(super) fn track_entity(&self, chunk: (i32, i32), entity: EntityId) {
        let revision = self.begin_routing_update();
        let previous_chunk = self.entity_chunk_unfenced(entity);
        self.update_chunk(chunk, |entities| {
            entities.insert(entity);
        });
        if let Some(previous_chunk) = previous_chunk.filter(|previous| *previous != chunk) {
            self.update_chunk(previous_chunk, |entities| {
                entities.remove(&entity);
            });
        }
        self.update_entity_chunk(entity, Some(chunk));
        self.finish_routing_update(revision);
    }

    pub(super) fn move_entity(
        &self,
        entity: EntityId,
        new_chunk: (i32, i32),
    ) -> Option<(i32, i32)> {
        let revision = self.begin_routing_update();
        let old_chunk = self.entity_chunk_unfenced(entity);
        if old_chunk == Some(new_chunk) {
            self.finish_routing_update(revision);
            return old_chunk;
        }
        self.update_chunk(new_chunk, |entities| {
            entities.insert(entity);
        });
        if let Some(old_chunk) = old_chunk {
            self.update_chunk(old_chunk, |entities| {
                entities.remove(&entity);
            });
        }
        self.update_entity_chunk(entity, Some(new_chunk));
        self.finish_routing_update(revision);
        old_chunk
    }

    pub(super) fn untrack_entity(&self, entity: EntityId) {
        let revision = self.begin_routing_update();
        if let Some(chunk) = self.entity_chunk_unfenced(entity) {
            self.update_chunk(chunk, |entities| {
                entities.remove(&entity);
            });
            self.update_entity_chunk(entity, None);
        }
        self.finish_routing_update(revision);
    }

    pub(super) fn entity_chunk(&self, entity: EntityId) -> Option<(i32, i32)> {
        self.read_routing(|| self.entity_chunk_unfenced(entity))
    }

    pub(super) fn entities_in_chunk(&self, chunk: (i32, i32)) -> Option<Arc<HashSet<EntityId>>> {
        self.read_routing(|| {
            self.entity_chunks[entity_index_shard(chunk)]
                .load()
                .get(&chunk)
                .cloned()
        })
    }

    pub(super) fn tracked_chunk_count(&self) -> usize {
        self.read_routing(|| {
            self.entity_chunks
                .iter()
                .map(|shard| shard.load().len())
                .sum()
        })
    }

    pub(super) fn all_entity_ids(&self) -> Vec<EntityId> {
        self.read_routing(|| {
            self.chunks_by_entity
                .iter()
                .flat_map(|shard| shard.load().keys().copied().collect::<Vec<_>>())
                .collect()
        })
    }

    pub(super) fn entity_candidates(
        &self,
        active_chunks: &HashSet<(i32, i32)>,
    ) -> HashSet<EntityId> {
        let mut candidates = HashSet::new();
        for shard in &self.entity_chunks {
            let chunks = shard.load();
            for (chunk, entities) in chunks.iter() {
                if active_chunks.contains(chunk) {
                    candidates.extend(entities.iter().copied());
                }
            }
        }
        candidates
    }

    pub(super) fn active_entity_candidates(&self) -> (Arc<HashSet<(i32, i32)>>, HashSet<EntityId>) {
        self.read_routing(|| {
            let active_chunks = self.active_chunks();
            let candidates = self.entity_candidates(active_chunks.as_ref());
            (active_chunks, candidates)
        })
    }

    pub(super) fn insert_terrain_pathing(&self, entities: impl IntoIterator<Item = EntityId>) {
        let entities = entities.into_iter().collect::<Vec<_>>();
        self.terrain_pathing_entities.rcu(|current| {
            let mut next = (**current).clone();
            let previous_len = next.len();
            next.extend(entities.iter().copied());
            if next.len() == previous_len {
                Arc::clone(current)
            } else {
                Arc::new(next)
            }
        });
    }

    pub(super) fn remove_terrain_pathing(&self, entities: impl IntoIterator<Item = EntityId>) {
        let entities = entities.into_iter().collect::<Vec<_>>();
        self.terrain_pathing_entities.rcu(|current| {
            let mut next = (**current).clone();
            let previous_len = next.len();
            for entity in &entities {
                next.remove(entity);
            }
            if next.len() == previous_len {
                Arc::clone(current)
            } else {
                Arc::new(next)
            }
        });
    }

    pub(super) fn terrain_pathing_entities(&self) -> Arc<HashSet<EntityId>> {
        self.terrain_pathing_entities.load_full()
    }

    fn update_chunk(&self, chunk: (i32, i32), update: impl FnOnce(&mut HashSet<EntityId>)) {
        let shard = &self.entity_chunks[entity_index_shard(chunk)];
        let current = shard.load_full();
        let mut next = (*current).clone();
        let mut entities = next
            .get(&chunk)
            .map(|entities| (**entities).clone())
            .unwrap_or_default();
        update(&mut entities);
        if entities.is_empty() {
            next.remove(&chunk);
        } else {
            next.insert(chunk, Arc::new(entities));
        }
        shard.store(Arc::new(next));
    }

    fn update_entity_chunk(&self, entity: EntityId, chunk: Option<(i32, i32)>) {
        let shard = &self.chunks_by_entity[entity_id_shard(entity)];
        let current = shard.load_full();
        let mut next = (*current).clone();
        if let Some(chunk) = chunk {
            next.insert(entity, chunk);
        } else {
            next.remove(&entity);
        }
        shard.store(Arc::new(next));
    }

    fn entity_chunk_unfenced(&self, entity: EntityId) -> Option<(i32, i32)> {
        self.chunks_by_entity[entity_id_shard(entity)]
            .load()
            .get(&entity)
            .copied()
    }

    fn read_routing<T>(&self, mut read: impl FnMut() -> T) -> T {
        loop {
            let revision = self
                .routing_revision
                .load(std::sync::atomic::Ordering::Acquire);
            if !revision.is_multiple_of(2) {
                std::hint::spin_loop();
                continue;
            }
            let value = read();
            if self
                .routing_revision
                .load(std::sync::atomic::Ordering::Acquire)
                == revision
            {
                return value;
            }
        }
    }

    fn begin_routing_update(&self) -> u64 {
        loop {
            let revision = self
                .routing_revision
                .load(std::sync::atomic::Ordering::Acquire);
            if revision.is_multiple_of(2)
                && self
                    .routing_revision
                    .compare_exchange_weak(
                        revision,
                        revision.wrapping_add(1),
                        std::sync::atomic::Ordering::AcqRel,
                        std::sync::atomic::Ordering::Acquire,
                    )
                    .is_ok()
            {
                return revision;
            }
            std::hint::spin_loop();
        }
    }

    fn finish_routing_update(&self, revision: u64) {
        self.routing_revision.store(
            revision.wrapping_add(2),
            std::sync::atomic::Ordering::Release,
        );
    }

    #[cfg(test)]
    pub(super) fn insert_chunk_candidate_for_test(&self, chunk: (i32, i32), entity: EntityId) {
        let revision = self.begin_routing_update();
        self.update_chunk(chunk, |entities| {
            entities.insert(entity);
        });
        self.finish_routing_update(revision);
    }
}

fn entity_index_shard(chunk: (i32, i32)) -> usize {
    let mixed = (chunk.0 as u32 as u64).wrapping_mul(0x9E37_79B1)
        ^ (chunk.1 as u32 as u64).wrapping_mul(0x85EB_CA77);
    (mixed as usize) & (ENTITY_INDEX_SHARDS - 1)
}

fn entity_id_shard(entity: EntityId) -> usize {
    (entity.0 as u32 as usize) & (ENTITY_INDEX_SHARDS - 1)
}
