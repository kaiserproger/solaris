use super::*;

const ENTITY_INDEX_SHARDS: usize = 64;
type ChunkEntityIndex = HashMap<(i32, i32), Arc<HashSet<EntityId>>>;
type EntityChunkIndex = HashMap<EntityId, (i32, i32)>;

#[derive(Debug, Clone, Copy)]
pub(super) struct ExpectedEntityRoutingMove {
    pub(super) entity: EntityId,
    pub(super) expected_chunk: (i32, i32),
    pub(super) new_chunk: (i32, i32),
}

#[derive(Debug, Clone, Copy)]
pub(super) struct EntityRoutingMoveOutcome {
    pub(super) entity: EntityId,
    pub(super) current_chunk: Option<(i32, i32)>,
    pub(super) applied: bool,
}

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
        self.move_entities(&[(entity, new_chunk)])
            .pop()
            .and_then(|(_, old_chunk)| old_chunk)
    }

    pub(super) fn move_entities(
        &self,
        moves: &[(EntityId, (i32, i32))],
    ) -> Vec<(EntityId, Option<(i32, i32)>)> {
        if moves.is_empty() {
            return Vec::new();
        }
        let revision = self.begin_routing_update();
        let mut previous_chunks = Vec::with_capacity(moves.len());
        let mut changed_chunks = HashMap::<(i32, i32), HashSet<EntityId>>::new();
        let mut changed_entities = HashMap::<EntityId, (i32, i32)>::new();
        for &(entity, new_chunk) in moves {
            let old_chunk = changed_entities
                .get(&entity)
                .copied()
                .or_else(|| self.entity_chunk_unfenced(entity));
            if old_chunk != Some(new_chunk) {
                self.staged_chunk_entities(&mut changed_chunks, new_chunk)
                    .insert(entity);
                if let Some(old_chunk) = old_chunk {
                    self.staged_chunk_entities(&mut changed_chunks, old_chunk)
                        .remove(&entity);
                }
                changed_entities.insert(entity, new_chunk);
            }
            previous_chunks.push((entity, old_chunk));
        }
        self.store_changed_chunks(changed_chunks);
        self.store_changed_entities(changed_entities);
        self.finish_routing_update(revision);
        previous_chunks
    }

    pub(super) fn move_entities_if_current(
        &self,
        moves: &[ExpectedEntityRoutingMove],
    ) -> Vec<EntityRoutingMoveOutcome> {
        if moves.is_empty() {
            return Vec::new();
        }
        let revision = self.begin_routing_update();
        let mut outcomes = Vec::with_capacity(moves.len());
        let mut changed_chunks = HashMap::<(i32, i32), HashSet<EntityId>>::new();
        let mut changed_entities = HashMap::<EntityId, (i32, i32)>::new();
        for &ExpectedEntityRoutingMove {
            entity,
            expected_chunk,
            new_chunk,
        } in moves
        {
            let current_chunk = changed_entities
                .get(&entity)
                .copied()
                .or_else(|| self.entity_chunk_unfenced(entity));
            let applied = current_chunk == Some(expected_chunk);
            if applied && current_chunk != Some(new_chunk) {
                self.staged_chunk_entities(&mut changed_chunks, new_chunk)
                    .insert(entity);
                self.staged_chunk_entities(&mut changed_chunks, expected_chunk)
                    .remove(&entity);
                changed_entities.insert(entity, new_chunk);
            }
            outcomes.push(EntityRoutingMoveOutcome {
                entity,
                current_chunk,
                applied,
            });
        }
        self.store_changed_chunks(changed_chunks);
        self.store_changed_entities(changed_entities);
        self.finish_routing_update(revision);
        outcomes
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

    fn staged_chunk_entities<'a>(
        &self,
        changed: &'a mut HashMap<(i32, i32), HashSet<EntityId>>,
        chunk: (i32, i32),
    ) -> &'a mut HashSet<EntityId> {
        changed.entry(chunk).or_insert_with(|| {
            self.entity_chunks[entity_index_shard(chunk)]
                .load()
                .get(&chunk)
                .map(|entities| (**entities).clone())
                .unwrap_or_default()
        })
    }

    fn store_changed_chunks(&self, changed: HashMap<(i32, i32), HashSet<EntityId>>) {
        let mut changed_shards = HashMap::<usize, ChunkEntityIndex>::new();
        for (chunk, entities) in changed {
            let shard_index = entity_index_shard(chunk);
            let shard = changed_shards
                .entry(shard_index)
                .or_insert_with(|| (*self.entity_chunks[shard_index].load_full()).clone());
            if entities.is_empty() {
                shard.remove(&chunk);
            } else {
                shard.insert(chunk, Arc::new(entities));
            }
        }
        for (shard_index, changed) in changed_shards {
            self.entity_chunks[shard_index].store(Arc::new(changed));
        }
    }

    fn store_changed_entities(&self, changed: HashMap<EntityId, (i32, i32)>) {
        let mut changed_shards = HashMap::<usize, EntityChunkIndex>::new();
        for (entity, chunk) in changed {
            let shard_index = entity_id_shard(entity);
            changed_shards
                .entry(shard_index)
                .or_insert_with(|| (*self.chunks_by_entity[shard_index].load_full()).clone())
                .insert(entity, chunk);
        }
        for (shard_index, changed) in changed_shards {
            self.chunks_by_entity[shard_index].store(Arc::new(changed));
        }
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
