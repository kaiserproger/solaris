use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock, RwLockWriteGuard};

use mc_data::Identifier;
use mc_data::block_light::BlockLightTable;

use crate::block::{BlockRegistry, BlockStateId};
use crate::chunk::{
    BlockMutationToken, BlockPos, ChestBlockEntity, Chunk, ChunkPos, FurnaceBlockEntity,
    HopperBlockEntity, ScheduledBlockTick, ScheduledFluidTick,
};
use crate::light::ChunkLight;
use crate::section::SECTION_DIM;
use crate::storage::{
    ChunkSnapshot, ScheduledTickView, WorldReadView, prune_incompatible_block_entities,
};

pub const WORLD_REGION_AXIS_CHUNKS: i32 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct WorldRegionPos {
    x: i32,
    z: i32,
}

#[derive(Default)]
struct ResidentRegion {
    chunks: HashMap<ChunkPos, ChunkSnapshot>,
    pending_journal_lsn: HashMap<ChunkPos, u64>,
}

#[derive(Clone)]
pub struct ResidentChunkStore {
    regions: Arc<RwLock<HashMap<WorldRegionPos, Arc<RwLock<ResidentRegion>>>>>,
    read_view: WorldReadView,
    scheduled_tick_view: ScheduledTickView,
    registry: Arc<BlockRegistry>,
}

#[derive(Clone)]
pub struct WorldMutationView {
    resident: ResidentChunkStore,
}

pub enum JournalStampResult {
    Stamped(Vec<ChunkSnapshot>),
    NewerDecision(u64),
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidentBlockMutation {
    Applied(BlockStateId),
    Missing,
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResidentBlockEdit {
    pub pos: BlockPos,
    pub new_state: BlockStateId,
    pub preserve_light: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResidentBlockPrecondition {
    pub pos: BlockPos,
    pub expected_state: BlockStateId,
    pub expected_token: BlockMutationToken,
}

pub struct ResidentFluidTickPlan<'a> {
    pub consumed_ticks: &'a [ScheduledFluidTick],
    pub edits: &'a [ResidentBlockEdit],
    pub preconditions: &'a [ResidentBlockPrecondition],
    pub scheduled_ticks: &'a [ScheduledFluidTick],
    pub light_table: Option<&'a BlockLightTable>,
    pub leaf_trigger_tick: Option<u64>,
}

pub struct ResidentScheduledBlockTickPlan<'a> {
    pub consumed_ticks: &'a [ScheduledBlockTick],
    pub edits: &'a [ResidentBlockEdit],
    pub preconditions: &'a [ResidentBlockPrecondition],
    pub light_table: Option<&'a BlockLightTable>,
    pub leaf_trigger_tick: Option<u64>,
}

/// A staged scheduled-block-tick change spanning more than one resident
/// region. The transaction owns immutable source snapshots and mutable staged
/// snapshots; no read or tick view is published until durability succeeds in
/// [`Self::commit_durably`].
pub struct ResidentCrossRegionScheduledBlockTickTransaction {
    resident: ResidentChunkStore,
    chunks: Vec<ResidentCrossRegionStagedChunk>,
    applied: Vec<ResidentAppliedBlockEdit>,
    touched: Vec<ChunkPos>,
    #[cfg(test)]
    publish_hook: Option<Arc<dyn Fn(ChunkPos) + Send + Sync>>,
}

struct ResidentCrossRegionStagedChunk {
    position: ChunkPos,
    expected: Option<ChunkSnapshot>,
    staged: Option<ChunkSnapshot>,
}

pub enum ResidentCrossRegionScheduledBlockTickPrepareResult {
    Prepared(ResidentCrossRegionScheduledBlockTickTransaction),
    Missing,
    Stale,
}

pub enum ResidentCrossRegionScheduledBlockTickCommitResult<E> {
    Applied(Vec<ResidentAppliedBlockEdit>),
    Missing,
    Stale,
    DurabilityFailed(E),
}

struct ResidentBlockEditPlan<'a> {
    edits: &'a [ResidentBlockEdit],
    preconditions: &'a [ResidentBlockPrecondition],
    scheduled_block_ticks: &'a [ScheduledBlockTick],
    consumed_block_ticks: &'a [ScheduledBlockTick],
    consumed_fluid_ticks: &'a [ScheduledFluidTick],
    scheduled_fluid_ticks: &'a [ScheduledFluidTick],
    light_table: Option<&'a BlockLightTable>,
    leaf_trigger_tick: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResidentAppliedBlockEdit {
    pub pos: BlockPos,
    pub previous: BlockStateId,
    pub new_state: BlockStateId,
    pub resulting_token: BlockMutationToken,
    pub previous_light: Option<ChunkLight>,
    pub changes_light: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResidentBlockEditBatchResult {
    Applied(Vec<ResidentAppliedBlockEdit>),
    Missing,
    Stale,
    CrossRegion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResidentChestCommitResult {
    Applied,
    Rejected(Vec<ChestBlockEntity>),
    Missing,
    CrossRegion,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResidentFurnaceCommitResult {
    Applied,
    Rejected(FurnaceBlockEntity),
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidentFurnaceTickCommitResult {
    Applied,
    Missing,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResidentBlockEntityChange<T> {
    pub position: BlockPos,
    pub expected: T,
    pub updated: T,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResidentHopperTransferPlan {
    pub expected_states: Vec<(BlockPos, BlockStateId)>,
    pub hoppers: Vec<ResidentBlockEntityChange<HopperBlockEntity>>,
    pub chests: Vec<ResidentBlockEntityChange<ChestBlockEntity>>,
    pub furnaces: Vec<ResidentBlockEntityChange<FurnaceBlockEntity>>,
    pub scheduled_block_ticks: Vec<ScheduledBlockTick>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidentHopperTransferCommitResult {
    Applied,
    Missing,
    Stale,
    CrossRegion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResidentOpaqueBlockEntityCommitResult {
    Applied,
    Missing,
    Stale,
}

impl ResidentChunkStore {
    pub(crate) fn new(
        read_view: WorldReadView,
        scheduled_tick_view: ScheduledTickView,
        registry: Arc<BlockRegistry>,
    ) -> Self {
        Self {
            regions: Arc::new(RwLock::new(HashMap::new())),
            read_view,
            scheduled_tick_view,
            registry,
        }
    }

    #[must_use]
    pub(crate) fn mutation_view(&self) -> WorldMutationView {
        WorldMutationView {
            resident: self.clone(),
        }
    }

    #[must_use]
    pub(crate) fn contains(&self, position: ChunkPos) -> bool {
        self.snapshot(position).is_some()
    }

    #[must_use]
    pub(crate) fn snapshot(&self, position: ChunkPos) -> Option<ChunkSnapshot> {
        self.read_view
            .publication_state()
            .read_consistent(|| self.snapshot_with_publication_excluded(position))
    }

    #[must_use]
    pub(crate) fn snapshots(&self) -> Vec<(ChunkPos, ChunkSnapshot)> {
        self.read_view.publication_state().read_consistent(|| {
            let regions: Vec<_> = self
                .regions
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .values()
                .cloned()
                .collect();
            let mut snapshots = Vec::new();
            for region in regions {
                snapshots.extend(
                    region
                        .read()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .chunks
                        .iter()
                        .map(|(&position, chunk)| (position, Arc::clone(chunk))),
                );
            }
            snapshots
        })
    }

    #[must_use]
    pub(crate) fn dirty_flush_snapshot(&self) -> (usize, Vec<(ChunkPos, ChunkSnapshot)>) {
        self.read_view.publication_state().read_consistent(|| {
            let regions: Vec<_> = self
                .regions
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .values()
                .cloned()
                .collect();
            let mut dirty_chunks = 0usize;
            let mut flushable = Vec::new();
            for region in regions {
                let region = region
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                for (&position, chunk) in &region.chunks {
                    if !chunk.dirty {
                        continue;
                    }
                    dirty_chunks += 1;
                    if !region.pending_journal_lsn.contains_key(&position) {
                        flushable.push((position, Arc::clone(chunk)));
                    }
                }
            }
            (dirty_chunks, flushable)
        })
    }

    #[must_use]
    pub(crate) fn has_flushable_dirty(&self) -> bool {
        self.read_view.publication_state().read_consistent(|| {
            let regions: Vec<_> = self
                .regions
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .values()
                .cloned()
                .collect();
            regions.into_iter().any(|region| {
                let region = region
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                region.chunks.iter().any(|(position, chunk)| {
                    chunk.dirty && !region.pending_journal_lsn.contains_key(position)
                })
            })
        })
    }

    pub(crate) fn stamp_world_journal_conditionally(
        &self,
        decision_id: u64,
        positions: &[ChunkPos],
    ) -> JournalStampResult {
        let publication = self.read_view.publication_state();
        let transaction = publication.transaction();
        let mut positions = positions.to_vec();
        positions.sort_unstable_by_key(|position| (position.x, position.z));
        positions.dedup();
        let mut region_positions = positions.iter().copied().map(region_of).collect::<Vec<_>>();
        region_positions.sort_unstable();
        region_positions.dedup();

        let regions = {
            let resident_regions = self
                .regions
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut regions = HashMap::with_capacity(region_positions.len());
            for region_position in region_positions {
                let Some(region) = resident_regions.get(&region_position) else {
                    return JournalStampResult::Missing;
                };
                regions.insert(region_position, Arc::clone(region));
            }
            regions
        };

        let mut newer_decision = decision_id;
        for position in &positions {
            let region = regions[&region_of(*position)]
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(chunk) = region.chunks.get(position) else {
                return JournalStampResult::Missing;
            };
            newer_decision = newer_decision.max(chunk.world_journal_lsn());
            if let Some(pending) = region.pending_journal_lsn.get(position) {
                newer_decision = newer_decision.max(*pending);
            }
        }
        if newer_decision > decision_id {
            return JournalStampResult::NewerDecision(newer_decision);
        }

        let mut snapshots = Vec::with_capacity(positions.len());
        let publishing = publication.begin_publish(transaction);
        for position in positions {
            let mut region = regions[&region_of(position)]
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let chunk = region
                .chunks
                .get_mut(&position)
                .expect("preflighted journal chunk");
            self.read_view.update_chunk(position, chunk, |chunk| {
                chunk.set_world_journal_lsn(decision_id)
            });
            region.pending_journal_lsn.insert(position, decision_id);
            snapshots.push(Arc::clone(
                region
                    .chunks
                    .get(&position)
                    .expect("stamped journal chunk remains resident"),
            ));
        }
        publishing.complete();
        JournalStampResult::Stamped(snapshots)
    }

    pub(crate) fn insert_if_absent(&self, position: ChunkPos, chunk: Chunk) -> bool {
        debug_assert_eq!(position, chunk.pos);
        let publication = self.read_view.publication_state();
        let _mutation = publication.mutation();
        let region = self.region_or_create(position);
        let mut region = region
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if region.chunks.contains_key(&position) {
            return false;
        }
        let chunk = Arc::new(chunk);
        region.chunks.insert(position, Arc::clone(&chunk));
        self.publish(position, &chunk);
        true
    }

    pub(crate) fn replace(&self, position: ChunkPos, chunk: ChunkSnapshot) {
        debug_assert_eq!(position, chunk.pos);
        let publication = self.read_view.publication_state();
        let _mutation = publication.mutation();
        let region = self.region_or_create(position);
        let mut region = region
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        region.pending_journal_lsn.remove(&position);
        region.chunks.insert(position, Arc::clone(&chunk));
        drop(region);
        self.publish(position, &chunk);
    }

    #[cfg(test)]
    pub(crate) fn replace_for_test(&self, position: ChunkPos, chunk: ChunkSnapshot) {
        self.replace(position, chunk);
    }

    pub(crate) fn remove_if_clean(&self, position: ChunkPos) -> bool {
        let publication = self.read_view.publication_state();
        let _mutation = publication.mutation();
        let Some(region) = self.region(position) else {
            return false;
        };
        let mut region = region
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if region.chunks.get(&position).is_none_or(|chunk| chunk.dirty) {
            return false;
        }
        region.chunks.remove(&position);
        region.pending_journal_lsn.remove(&position);
        self.read_view.remove_chunk(position);
        self.scheduled_tick_view.remove_chunk(position);
        true
    }

    pub(crate) fn mutate<R>(
        &self,
        position: ChunkPos,
        update: impl FnOnce(&mut Chunk) -> R,
    ) -> Option<R> {
        let publication = self.read_view.publication_state();
        let _mutation = publication.mutation();
        let region = self.region(position)?;
        let mut region = region
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let chunk = region.chunks.get_mut(&position)?;
        let result = self.read_view.update_chunk(position, chunk, update);
        self.read_view.publish_furnaces(position, chunk);
        self.scheduled_tick_view
            .publish_chunk(position, chunk, &self.registry);
        Some(result)
    }

    pub(crate) fn install_region_flush<E>(
        &self,
        planned: &[(ChunkPos, u64, ChunkSnapshot)],
        install: impl FnOnce() -> Result<(), E>,
    ) -> Result<bool, E> {
        if planned.is_empty() {
            return Ok(false);
        }
        let publication = self.read_view.publication_state();
        let _mutation = publication.mutation();
        let mut region_keys = planned
            .iter()
            .map(|(position, _, _)| region_of(*position))
            .collect::<Vec<_>>();
        region_keys.sort_unstable();
        region_keys.dedup();
        let region_handles = {
            let regions = self
                .regions
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(handles) = region_keys
                .iter()
                .map(|key| regions.get(key).cloned())
                .collect::<Option<Vec<_>>>()
            else {
                return Ok(false);
            };
            handles
        };
        let regions = region_handles
            .iter()
            .map(|region| {
                region
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
            })
            .collect::<Vec<_>>();
        let current = planned
            .iter()
            .all(|(position, dirty_generation, snapshot)| {
                let region_index = region_keys
                    .binary_search(&region_of(*position))
                    .expect("planned resident region is locked");
                let region = &regions[region_index];
                !region.pending_journal_lsn.contains_key(position)
                    && region.chunks.get(position).is_some_and(|chunk| {
                        chunk.dirty
                            && (*dirty_generation == 0
                                || chunk.dirty_generation == *dirty_generation)
                            && Arc::ptr_eq(chunk, snapshot)
                    })
            });
        if !current {
            return Ok(false);
        }

        install()?;
        Ok(true)
    }

    pub(crate) fn install_region_snapshot_flush<E>(
        &self,
        planned: &[(ChunkPos, u64, ChunkSnapshot)],
        install: impl FnOnce() -> Result<(), E>,
    ) -> Result<(), E> {
        if planned.is_empty() {
            return Ok(());
        }
        let publication = self.read_view.publication_state();
        let _mutation = publication.mutation();
        let mut region_keys = planned
            .iter()
            .map(|(position, _, _)| region_of(*position))
            .collect::<Vec<_>>();
        region_keys.sort_unstable();
        region_keys.dedup();
        let region_handles = {
            let regions = self
                .regions
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            region_keys
                .iter()
                .filter_map(|key| regions.get(key).cloned())
                .collect::<Vec<_>>()
        };
        let _regions = region_handles
            .iter()
            .map(|region| {
                region
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
            })
            .collect::<Vec<_>>();
        install()
    }

    pub(crate) fn finalize_region_flush(
        &self,
        planned: Vec<(ChunkPos, u64, ChunkSnapshot)>,
    ) -> usize {
        if planned.is_empty() {
            return 0;
        }
        let publication = self.read_view.publication_state();
        let _mutation = publication.mutation();
        let mut region_keys = planned
            .iter()
            .map(|(position, _, _)| region_of(*position))
            .collect::<Vec<_>>();
        region_keys.sort_unstable();
        region_keys.dedup();
        let region_handles = {
            let regions = self
                .regions
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            region_keys
                .iter()
                .map(|key| regions.get(key).cloned())
                .collect::<Vec<_>>()
        };
        let mut regions = region_handles
            .iter()
            .map(|region| {
                region.as_ref().map(|region| {
                    region
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                })
            })
            .collect::<Vec<_>>();
        let mut cleaned = 0usize;
        for (position, dirty_generation, snapshot) in planned {
            let region_index = region_keys
                .binary_search(&region_of(position))
                .expect("planned resident region is represented");
            let Some(region) = regions[region_index].as_mut() else {
                continue;
            };
            if region.pending_journal_lsn.contains_key(&position) {
                continue;
            }
            let Some(chunk) = region.chunks.get_mut(&position) else {
                continue;
            };
            if !chunk.dirty
                || (dirty_generation != 0 && chunk.dirty_generation != dirty_generation)
                || !Arc::ptr_eq(chunk, &snapshot)
            {
                continue;
            }
            drop(snapshot);
            self.read_view
                .update_chunk_snapshot(position, chunk, |chunk| {
                    Arc::make_mut(chunk).dirty = false;
                });
            cleaned += 1;
        }
        cleaned
    }

    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.read_view.resident_len()
    }

    #[must_use]
    pub(crate) fn dirty_count(&self) -> usize {
        self.read_view.dirty_len()
    }

    fn publish(&self, position: ChunkPos, chunk: &ChunkSnapshot) {
        self.read_view.publish_chunk(position, Arc::clone(chunk));
        self.scheduled_tick_view
            .publish_chunk(position, chunk, &self.registry);
    }

    fn region(&self, position: ChunkPos) -> Option<Arc<RwLock<ResidentRegion>>> {
        self.regions
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&region_of(position))
            .cloned()
    }

    fn snapshot_with_publication_excluded(&self, position: ChunkPos) -> Option<ChunkSnapshot> {
        let region = self.region(position)?;
        region
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .chunks
            .get(&position)
            .map(Arc::clone)
    }

    fn region_or_create(&self, position: ChunkPos) -> Arc<RwLock<ResidentRegion>> {
        let key = region_of(position);
        if let Some(region) = self
            .regions
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&key)
            .cloned()
        {
            return region;
        }
        self.regions
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(key)
            .or_insert_with(|| Arc::new(RwLock::new(ResidentRegion::default())))
            .clone()
    }
}

impl WorldMutationView {
    /// Apply accumulated active-player time to resident chunks.
    ///
    /// Callers batch ticks before entering this mutation boundary so ordinary
    /// simulation does not republish every active chunk on every game tick.
    pub fn increment_chunk_inhabited_times(
        &self,
        updates: &[(ChunkPos, u64)],
    ) -> Vec<(ChunkPos, u64)> {
        let mut missing = Vec::new();
        for &(position, elapsed_ticks) in updates {
            if elapsed_ticks != 0
                && self
                    .resident
                    .mutate(position, |chunk| {
                        chunk.increment_inhabited_time(elapsed_ticks);
                    })
                    .is_none()
            {
                missing.push((position, elapsed_ticks));
            }
        }
        missing
    }

    pub fn stamp_chunks_for_world_journal(
        &self,
        decision_id: u64,
        positions: &[ChunkPos],
    ) -> crate::JournalStampResult {
        self.resident
            .stamp_world_journal_conditionally(decision_id, positions)
    }

    /// Release a chunk's flush fence only after the matching journal decision is durable.
    pub fn clear_journal_pending_conditionally(
        &self,
        decision_id: u64,
        positions: &[ChunkPos],
    ) -> usize {
        let publication = self.resident.read_view.publication_state();
        let _mutation = publication.mutation();
        let mut cleared = 0;
        for &position in positions {
            let Some(region) = self.resident.region(position) else {
                continue;
            };
            let mut region = region
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if region.pending_journal_lsn.get(&position) != Some(&decision_id)
                || region
                    .chunks
                    .get(&position)
                    .is_none_or(|chunk| chunk.world_journal_lsn() != decision_id)
            {
                continue;
            }
            region.pending_journal_lsn.remove(&position);
            cleared += 1;
        }
        if cleared > 0 {
            self.resident.read_view.notify_dirty_flush();
        }
        cleared
    }

    pub fn publish_baked_light_conditionally<'a>(
        &self,
        expected_sources: &HashMap<ChunkPos, Option<ChunkSnapshot>>,
        updates: impl IntoIterator<Item = (ChunkPos, &'a ChunkLight)>,
    ) -> bool {
        let publication = self.resident.read_view.publication_state();
        let transaction = publication.transaction();
        let updates = updates.into_iter().collect::<Vec<_>>();
        for (position, expected) in expected_sources {
            let region = self.resident.region(*position);
            let current = region.as_ref().and_then(|region| {
                region
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .chunks
                    .get(position)
                    .map(Arc::clone)
            });
            let is_current = match (expected, &current) {
                (Some(expected), Some(current)) => Arc::ptr_eq(expected, current),
                (None, None) => true,
                _ => false,
            };
            if !is_current {
                return false;
            }
        }
        if updates.iter().any(|(position, _)| {
            self.resident
                .snapshot_with_publication_excluded(*position)
                .is_none()
        }) {
            return false;
        }

        let publishing = publication.begin_publish(transaction);
        for (position, light) in updates {
            let region = self
                .resident
                .region(position)
                .expect("preflighted light region");
            let mut region = region
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let chunk = region
                .chunks
                .get_mut(&position)
                .expect("preflighted baked-light chunk");
            self.resident
                .read_view
                .update_chunk(position, chunk, |chunk| chunk.set_baked_light(light));
        }
        publishing.complete();
        true
    }

    /// Rare fallback after optimistic light publication loses a race.
    /// Keeps one current source snapshot and its baked-light writes under the
    /// same sorted regional locks, so recomputation is bounded and final.
    pub fn recompute_and_publish_baked_light<T, I, F, G>(
        &self,
        source_positions: I,
        recompute: F,
        light_of: G,
    ) -> Vec<T>
    where
        I: IntoIterator<Item = ChunkPos>,
        F: FnOnce(&HashMap<ChunkPos, Option<ChunkSnapshot>>) -> Vec<T>,
        G: for<'a> Fn(&'a T) -> (ChunkPos, &'a ChunkLight),
    {
        let publication = self.resident.read_view.publication_state();
        let transaction = publication.transaction();
        let mut source_positions = source_positions.into_iter().collect::<Vec<_>>();
        source_positions.sort_unstable_by_key(|position| (position.x, position.z));
        source_positions.dedup();
        let sources = source_positions
            .into_iter()
            .map(|position| {
                (
                    position,
                    self.resident.snapshot_with_publication_excluded(position),
                )
            })
            .collect::<HashMap<_, _>>();
        let updates = recompute(&sources);
        let mut published = Vec::with_capacity(updates.len());
        let publishing = publication.begin_publish(transaction);
        for update in updates {
            let (position, light) = light_of(&update);
            let Some(region) = self.resident.region(position) else {
                continue;
            };
            let mut region = region
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(chunk) = region.chunks.get_mut(&position) else {
                continue;
            };
            self.resident
                .read_view
                .update_chunk(position, chunk, |chunk| chunk.set_baked_light(light));
            published.push(update);
        }
        publishing.complete();
        published
    }

    pub fn commit_opaque_block_entity_conditionally(
        &self,
        position: BlockPos,
        expected_state: BlockStateId,
        expected_token: BlockMutationToken,
        bytes: Vec<u8>,
    ) -> ResidentOpaqueBlockEntityCommitResult {
        self.commit_opaque_block_entity_conditionally_inner(
            None,
            position,
            expected_state,
            expected_token,
            bytes,
        )
        .0
    }

    pub fn commit_opaque_block_entity_conditionally_journaled(
        &self,
        decision_id: u64,
        position: BlockPos,
        expected_state: BlockStateId,
        expected_token: BlockMutationToken,
        bytes: Vec<u8>,
    ) -> (ResidentOpaqueBlockEntityCommitResult, Vec<ChunkPos>) {
        self.commit_opaque_block_entity_conditionally_inner(
            Some(decision_id),
            position,
            expected_state,
            expected_token,
            bytes,
        )
    }

    fn commit_opaque_block_entity_conditionally_inner(
        &self,
        decision_id: Option<u64>,
        position: BlockPos,
        expected_state: BlockStateId,
        expected_token: BlockMutationToken,
        bytes: Vec<u8>,
    ) -> (ResidentOpaqueBlockEntityCommitResult, Vec<ChunkPos>) {
        let publication = self.resident.read_view.publication_state();
        let _mutation = publication.mutation();
        let chunk_position = chunk_pos_of(position);
        let Some(region) = self.resident.region(chunk_position) else {
            return (ResidentOpaqueBlockEntityCommitResult::Missing, Vec::new());
        };
        let mut region = region
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(chunk) = region.chunks.get_mut(&chunk_position) else {
            return (ResidentOpaqueBlockEntityCommitResult::Missing, Vec::new());
        };
        let local_x = position.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = position.z.rem_euclid(SECTION_DIM as i32) as u8;
        if chunk.get_block(local_x, position.y, local_z) != Some(expected_state)
            || chunk.block_mutation_token(local_x, position.y, local_z) != Some(expected_token)
        {
            return (ResidentOpaqueBlockEntityCommitResult::Stale, Vec::new());
        }
        let changed = self
            .resident
            .read_view
            .update_chunk(chunk_position, chunk, |chunk| {
                if chunk.block_entities.get(&position) != Some(&bytes) {
                    chunk.block_entities.insert(position, bytes);
                    chunk.mark_dirty();
                    if let Some(decision_id) = decision_id {
                        chunk.set_world_journal_lsn(decision_id);
                    }
                    true
                } else {
                    false
                }
            });
        if changed && let Some(decision_id) = decision_id {
            region
                .pending_journal_lsn
                .insert(chunk_position, decision_id);
        }
        (
            ResidentOpaqueBlockEntityCommitResult::Applied,
            changed.then_some(chunk_position).into_iter().collect(),
        )
    }

    #[must_use]
    pub fn furnace_block_entity(&self, position: BlockPos) -> Option<FurnaceBlockEntity> {
        let chunk = self.resident.snapshot(chunk_pos_of(position))?;
        Some(chunk.furnaces.get(&position).cloned().unwrap_or_default())
    }

    #[must_use]
    pub fn furnace_tick_snapshot(
        &self,
        position: BlockPos,
    ) -> Option<(BlockStateId, FurnaceBlockEntity)> {
        let chunk = self.resident.snapshot(chunk_pos_of(position))?;
        let local_x = position.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = position.z.rem_euclid(SECTION_DIM as i32) as u8;
        Some((
            chunk.get_block(local_x, position.y, local_z)?,
            chunk.furnaces.get(&position).cloned().unwrap_or_default(),
        ))
    }

    pub fn backfill_hopper_ticks(&self, positions: &[ChunkPos], trigger_tick: u64) -> usize {
        let publication = self.resident.read_view.publication_state();
        let _mutation = publication.mutation();
        let mut grouped = HashMap::<WorldRegionPos, Vec<ChunkPos>>::new();
        for &position in positions {
            grouped
                .entry(region_of(position))
                .or_default()
                .push(position);
        }
        let mut grouped = grouped.into_iter().collect::<Vec<_>>();
        grouped.sort_unstable_by_key(|(region, _)| *region);

        let mut scheduled = 0;
        for (region_position, mut positions) in grouped {
            positions.sort_unstable_by_key(|position| (position.x, position.z));
            positions.dedup();
            let Some(region) = self
                .resident
                .regions
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&region_position)
                .cloned()
            else {
                continue;
            };
            let mut region = region
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for chunk_position in positions {
                let Some(chunk) = region.chunks.get_mut(&chunk_position) else {
                    continue;
                };
                let mut hopper_positions = chunk.hoppers.keys().copied().collect::<Vec<_>>();
                hopper_positions
                    .sort_unstable_by_key(|position| (position.x, position.y, position.z));
                let ticks = hopper_positions
                    .into_iter()
                    .filter_map(|position| {
                        let local_x = position.x.rem_euclid(SECTION_DIM as i32) as u8;
                        let local_z = position.z.rem_euclid(SECTION_DIM as i32) as u8;
                        let state_id = chunk.get_block(local_x, position.y, local_z)?;
                        let state = self.resident.registry.by_id(state_id)?;
                        (state.block.id.path() == "hopper"
                            && !chunk
                                .scheduled_block_ticks()
                                .iter()
                                .any(|tick| tick.pos == position && tick.block == state.block.id))
                        .then(|| {
                            ScheduledBlockTick::new(
                                position,
                                state.block.id.clone(),
                                trigger_tick,
                                0,
                            )
                        })
                    })
                    .collect::<Vec<_>>();
                if ticks.is_empty() {
                    continue;
                }
                scheduled += self
                    .resident
                    .read_view
                    .update_chunk(chunk_position, chunk, |chunk| {
                        ticks
                            .into_iter()
                            .filter(|tick| chunk.schedule_block_tick(tick.clone()))
                            .count()
                    });
                self.resident.scheduled_tick_view.publish_chunk(
                    chunk_position,
                    chunk,
                    &self.resident.registry,
                );
            }
        }
        scheduled
    }

    pub fn commit_furnace_tick_conditionally(
        &self,
        position: BlockPos,
        expected_state: BlockStateId,
        updated_state: BlockStateId,
        expected: &FurnaceBlockEntity,
        updated: &FurnaceBlockEntity,
    ) -> ResidentFurnaceTickCommitResult {
        self.commit_furnace_tick_conditionally_inner(
            None,
            position,
            expected_state,
            updated_state,
            expected,
            updated,
        )
        .0
    }

    pub fn commit_furnace_tick_conditionally_journaled(
        &self,
        decision_id: u64,
        position: BlockPos,
        expected_state: BlockStateId,
        updated_state: BlockStateId,
        expected: &FurnaceBlockEntity,
        updated: &FurnaceBlockEntity,
    ) -> (ResidentFurnaceTickCommitResult, Vec<ChunkPos>) {
        self.commit_furnace_tick_conditionally_inner(
            Some(decision_id),
            position,
            expected_state,
            updated_state,
            expected,
            updated,
        )
    }

    fn commit_furnace_tick_conditionally_inner(
        &self,
        decision_id: Option<u64>,
        position: BlockPos,
        expected_state: BlockStateId,
        updated_state: BlockStateId,
        expected: &FurnaceBlockEntity,
        updated: &FurnaceBlockEntity,
    ) -> (ResidentFurnaceTickCommitResult, Vec<ChunkPos>) {
        let publication = self.resident.read_view.publication_state();
        let _mutation = publication.mutation();
        let chunk_position = chunk_pos_of(position);
        let Some(region) = self.resident.region(chunk_position) else {
            return (ResidentFurnaceTickCommitResult::Missing, Vec::new());
        };
        let mut region = region
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(chunk) = region.chunks.get_mut(&chunk_position) else {
            return (ResidentFurnaceTickCommitResult::Missing, Vec::new());
        };
        let local_x = position.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = position.z.rem_euclid(SECTION_DIM as i32) as u8;
        let current = chunk.furnaces.get(&position).cloned().unwrap_or_default();
        if chunk.get_block(local_x, position.y, local_z) != Some(expected_state)
            || &current != expected
        {
            return (ResidentFurnaceTickCommitResult::Stale, Vec::new());
        }
        let air = self
            .resident
            .registry
            .block(&Identifier::parse("minecraft:air").expect("static identifier"))
            .map(|block| block.default)
            .unwrap_or(BlockStateId(0));
        let changed = self
            .resident
            .read_view
            .update_chunk(chunk_position, chunk, |chunk| {
                let mut changed = false;
                if expected_state != updated_state {
                    let previous = chunk
                        .set_block_and_update_retaining_baked_light(
                            local_x,
                            position.y,
                            local_z,
                            updated_state,
                            air,
                        )
                        .expect("validated furnace position remains in chunk");
                    changed |= previous != updated_state;
                }
                if chunk.furnaces.get(&position) != Some(updated) {
                    chunk.furnaces.insert(position, updated.clone());
                    changed = true;
                }
                if changed {
                    chunk.mark_dirty();
                    if let Some(decision_id) = decision_id {
                        chunk.set_world_journal_lsn(decision_id);
                    }
                }
                changed
            });
        self.resident
            .read_view
            .publish_furnaces(chunk_position, chunk);
        if changed && let Some(decision_id) = decision_id {
            region
                .pending_journal_lsn
                .insert(chunk_position, decision_id);
        }
        (
            ResidentFurnaceTickCommitResult::Applied,
            changed.then_some(chunk_position).into_iter().collect(),
        )
    }

    pub fn commit_hopper_transfer_conditionally(
        &self,
        plan: &ResidentHopperTransferPlan,
    ) -> ResidentHopperTransferCommitResult {
        self.commit_hopper_transfer_conditionally_inner(None, &[], plan)
            .0
    }

    pub fn commit_scheduled_hopper_transfer_conditionally(
        &self,
        consumed_ticks: &[ScheduledBlockTick],
        plan: &ResidentHopperTransferPlan,
    ) -> ResidentHopperTransferCommitResult {
        self.commit_hopper_transfer_conditionally_inner(None, consumed_ticks, plan)
            .0
    }

    pub fn commit_scheduled_hopper_transfer_conditionally_journaled(
        &self,
        decision_id: u64,
        consumed_ticks: &[ScheduledBlockTick],
        plan: &ResidentHopperTransferPlan,
    ) -> (ResidentHopperTransferCommitResult, Vec<ChunkPos>) {
        self.commit_hopper_transfer_conditionally_inner(Some(decision_id), consumed_ticks, plan)
    }

    fn commit_hopper_transfer_conditionally_inner(
        &self,
        decision_id: Option<u64>,
        consumed_ticks: &[ScheduledBlockTick],
        plan: &ResidentHopperTransferPlan,
    ) -> (ResidentHopperTransferCommitResult, Vec<ChunkPos>) {
        let publication = self.resident.read_view.publication_state();
        let transaction = publication.transaction();
        let mut positions = hopper_transfer_positions(plan);
        positions.extend(consumed_ticks.iter().map(|tick| tick.pos));
        positions.sort_unstable_by_key(|position| (position.x, position.y, position.z));
        positions.dedup();
        if positions.is_empty() {
            return (ResidentHopperTransferCommitResult::Missing, Vec::new());
        }
        let mut staged = HashMap::<ChunkPos, ChunkSnapshot>::new();
        for chunk_position in positions.iter().map(|position| chunk_pos_of(*position)) {
            if staged.contains_key(&chunk_position) {
                continue;
            }
            let Some(chunk) = self
                .resident
                .snapshot_with_publication_excluded(chunk_position)
            else {
                return (ResidentHopperTransferCommitResult::Missing, Vec::new());
            };
            staged.insert(chunk_position, Arc::new((*chunk).clone()));
        }
        for (position, expected) in &plan.expected_states {
            let chunk_position = chunk_pos_of(*position);
            let chunk = &staged[&chunk_position];
            let local_x = position.x.rem_euclid(SECTION_DIM as i32) as u8;
            let local_z = position.z.rem_euclid(SECTION_DIM as i32) as u8;
            if chunk.get_block(local_x, position.y, local_z) != Some(*expected) {
                return (ResidentHopperTransferCommitResult::Stale, Vec::new());
            }
        }
        if plan.hoppers.iter().any(|change| {
            let chunk_position = chunk_pos_of(change.position);
            staged[&chunk_position]
                .hoppers
                .get(&change.position)
                .cloned()
                .unwrap_or_default()
                != change.expected
        }) || plan.chests.iter().any(|change| {
            let chunk_position = chunk_pos_of(change.position);
            staged[&chunk_position]
                .chests
                .get(&change.position)
                .cloned()
                .unwrap_or_default()
                != change.expected
        }) || plan.furnaces.iter().any(|change| {
            let chunk_position = chunk_pos_of(change.position);
            staged[&chunk_position]
                .furnaces
                .get(&change.position)
                .cloned()
                .unwrap_or_default()
                != change.expected
        }) {
            return (ResidentHopperTransferCommitResult::Stale, Vec::new());
        }
        let mut consumed_by_chunk = HashMap::<ChunkPos, Vec<ScheduledBlockTick>>::new();
        for tick in consumed_ticks {
            consumed_by_chunk
                .entry(chunk_pos_of(tick.pos))
                .or_default()
                .push(tick.clone());
        }
        if consumed_by_chunk.iter().any(|(position, expected)| {
            !staged[position]
                .scheduled_block_ticks()
                .starts_with(expected)
        }) {
            return (ResidentHopperTransferCommitResult::Stale, Vec::new());
        }

        let mut changed_chunks = HashSet::new();
        let mut consumed_by_chunk = consumed_by_chunk.into_iter().collect::<Vec<_>>();
        consumed_by_chunk.sort_unstable_by_key(|(position, _)| (position.x, position.z));
        for (position, expected) in consumed_by_chunk {
            let chunk = Arc::make_mut(staged.get_mut(&position).expect("staged hopper tick chunk"));
            assert!(chunk.drain_scheduled_block_tick_prefix(&expected));
            if let Some(decision_id) = decision_id {
                chunk.set_world_journal_lsn(decision_id);
            }
            changed_chunks.insert(position);
        }
        for change in &plan.hoppers {
            let position = chunk_pos_of(change.position);
            let chunk = Arc::make_mut(staged.get_mut(&position).expect("hopper chunk"));
            chunk
                .hoppers
                .insert(change.position, change.updated.clone());
            chunk.mark_dirty();
            if let Some(decision_id) = decision_id {
                chunk.set_world_journal_lsn(decision_id);
            }
            changed_chunks.insert(position);
        }
        for change in &plan.chests {
            let position = chunk_pos_of(change.position);
            let chunk = Arc::make_mut(staged.get_mut(&position).expect("chest chunk"));
            chunk.chests.insert(change.position, change.updated.clone());
            chunk.mark_dirty();
            if let Some(decision_id) = decision_id {
                chunk.set_world_journal_lsn(decision_id);
            }
            changed_chunks.insert(position);
        }
        for change in &plan.furnaces {
            let position = chunk_pos_of(change.position);
            let chunk = Arc::make_mut(staged.get_mut(&position).expect("furnace chunk"));
            chunk
                .furnaces
                .insert(change.position, change.updated.clone());
            chunk.mark_dirty();
            if let Some(decision_id) = decision_id {
                chunk.set_world_journal_lsn(decision_id);
            }
            changed_chunks.insert(position);
        }
        for tick in &plan.scheduled_block_ticks {
            let position = chunk_pos_of(tick.pos);
            let chunk = Arc::make_mut(staged.get_mut(&position).expect("scheduled-tick chunk"));
            if chunk.schedule_block_tick(tick.clone()) {
                if let Some(decision_id) = decision_id {
                    chunk.set_world_journal_lsn(decision_id);
                }
                changed_chunks.insert(position);
            }
        }
        let mut changed_chunks = changed_chunks.into_iter().collect::<Vec<_>>();
        changed_chunks.sort_unstable_by_key(|position| (position.x, position.z));
        let publishing = publication.begin_publish(transaction);
        for &position in &changed_chunks {
            let chunk = Arc::clone(&staged[&position]);
            let region = self
                .resident
                .region(position)
                .expect("staged hopper region");
            let mut region = region
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            region.chunks.insert(position, Arc::clone(&chunk));
            if let Some(decision_id) = decision_id {
                region.pending_journal_lsn.insert(position, decision_id);
            }
            drop(region);
            self.resident.publish(position, &chunk);
            self.resident.read_view.publish_furnaces(position, &chunk);
        }
        publishing.complete();
        (ResidentHopperTransferCommitResult::Applied, changed_chunks)
    }

    pub fn commit_furnace_conditionally(
        &self,
        position: BlockPos,
        expected: &FurnaceBlockEntity,
        updated: &FurnaceBlockEntity,
    ) -> ResidentFurnaceCommitResult {
        let publication = self.resident.read_view.publication_state();
        let _mutation = publication.mutation();
        let chunk_position = chunk_pos_of(position);
        let Some(region) = self.resident.region(chunk_position) else {
            return ResidentFurnaceCommitResult::Missing;
        };
        let mut region = region
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(chunk) = region.chunks.get_mut(&chunk_position) else {
            return ResidentFurnaceCommitResult::Missing;
        };
        let authoritative = chunk.furnaces.get(&position).cloned().unwrap_or_default();
        if authoritative.slots != expected.slots
            || authoritative.recipes_used != expected.recipes_used
        {
            return ResidentFurnaceCommitResult::Rejected(authoritative);
        }
        let mut merged = authoritative;
        merged.slots = updated.slots.clone();
        merged.recipes_used = updated.recipes_used.clone();
        self.resident
            .read_view
            .update_chunk(chunk_position, chunk, |chunk| {
                if chunk.furnaces.get(&position) != Some(&merged) {
                    chunk.furnaces.insert(position, merged);
                    chunk.mark_dirty();
                }
            });
        self.resident
            .read_view
            .publish_furnaces(chunk_position, chunk);
        ResidentFurnaceCommitResult::Applied
    }

    #[must_use]
    pub fn chest_block_entities(&self, positions: &[BlockPos]) -> Option<Vec<ChestBlockEntity>> {
        self.resident
            .read_view
            .publication_state()
            .read_consistent(|| {
                let first = *positions.first()?;
                let owner = region_of(chunk_pos_of(first));
                let mut unique = HashSet::with_capacity(positions.len());
                if positions.iter().any(|position| {
                    !unique.insert(*position) || region_of(chunk_pos_of(*position)) != owner
                }) {
                    return None;
                }
                let region = self.resident.region(chunk_pos_of(first))?;
                let region = region
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                positions
                    .iter()
                    .map(|position| {
                        region
                            .chunks
                            .get(&chunk_pos_of(*position))
                            .map(|chunk| chunk.chests.get(position).cloned().unwrap_or_default())
                    })
                    .collect()
            })
    }

    pub fn commit_chests_conditionally(
        &self,
        positions: &[BlockPos],
        expected: &[ChestBlockEntity],
        updated: &[ChestBlockEntity],
    ) -> ResidentChestCommitResult {
        let publication = self.resident.read_view.publication_state();
        let _mutation = publication.mutation();
        let mut unique = HashSet::with_capacity(positions.len());
        if positions.is_empty()
            || positions.len() != expected.len()
            || positions.len() != updated.len()
            || positions.iter().any(|position| !unique.insert(*position))
        {
            return ResidentChestCommitResult::Missing;
        }
        let owner = region_of(chunk_pos_of(positions[0]));
        if positions
            .iter()
            .any(|position| region_of(chunk_pos_of(*position)) != owner)
        {
            return ResidentChestCommitResult::CrossRegion;
        }
        let Some(region) = self.resident.region(chunk_pos_of(positions[0])) else {
            return ResidentChestCommitResult::Missing;
        };
        let mut region = region
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if positions
            .iter()
            .any(|position| !region.chunks.contains_key(&chunk_pos_of(*position)))
        {
            return ResidentChestCommitResult::Missing;
        }
        let authoritative = positions
            .iter()
            .map(|position| {
                region.chunks[&chunk_pos_of(*position)]
                    .chests
                    .get(position)
                    .cloned()
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        if authoritative != expected {
            return ResidentChestCommitResult::Rejected(authoritative);
        }

        let mut by_chunk = HashMap::<ChunkPos, Vec<(BlockPos, ChestBlockEntity)>>::new();
        for (&position, chest) in positions.iter().zip(updated) {
            by_chunk
                .entry(chunk_pos_of(position))
                .or_default()
                .push((position, chest.clone()));
        }
        for (chunk_position, updates) in by_chunk {
            let chunk = region
                .chunks
                .get_mut(&chunk_position)
                .expect("preflighted resident chest chunk");
            self.resident
                .read_view
                .update_chunk(chunk_position, chunk, move |chunk| {
                    let mut changed = false;
                    for (position, chest) in updates {
                        if chunk.chests.get(&position) != Some(&chest) {
                            chunk.chests.insert(position, chest);
                            changed = true;
                        }
                    }
                    if changed {
                        chunk.mark_dirty();
                    }
                });
        }
        ResidentChestCommitResult::Applied
    }

    pub fn schedule_fluid_ticks(&self, ticks: &[ScheduledFluidTick]) -> usize {
        let publication = self.resident.read_view.publication_state();
        let _mutation = publication.mutation();
        let mut grouped =
            HashMap::<WorldRegionPos, HashMap<ChunkPos, Vec<ScheduledFluidTick>>>::new();
        for tick in ticks {
            let chunk = chunk_pos_of(tick.pos);
            grouped
                .entry(region_of(chunk))
                .or_default()
                .entry(chunk)
                .or_default()
                .push(tick.clone());
        }

        let mut scheduled = 0;
        for (region_position, chunks) in grouped {
            let Some(region) = self
                .resident
                .regions
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&region_position)
                .cloned()
            else {
                continue;
            };
            let mut region = region
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for (chunk_position, ticks) in chunks {
                let Some(chunk) = region.chunks.get_mut(&chunk_position) else {
                    continue;
                };
                scheduled +=
                    self.resident
                        .read_view
                        .update_chunk(chunk_position, chunk, move |chunk| {
                            chunk.schedule_fluid_tick_batch(ticks)
                        });
                self.resident.scheduled_tick_view.publish_chunk(
                    chunk_position,
                    chunk,
                    &self.resident.registry,
                );
            }
        }
        scheduled
    }

    pub fn apply_block_edits_conditionally(
        &self,
        edits: &[ResidentBlockEdit],
        preconditions: &[ResidentBlockPrecondition],
        scheduled_block_ticks: &[ScheduledBlockTick],
        light_table: Option<&BlockLightTable>,
        leaf_trigger_tick: Option<u64>,
    ) -> ResidentBlockEditBatchResult {
        self.apply_block_edits_conditionally_inner(
            None,
            ResidentBlockEditPlan {
                edits,
                preconditions,
                scheduled_block_ticks,
                consumed_block_ticks: &[],
                consumed_fluid_ticks: &[],
                scheduled_fluid_ticks: &[],
                light_table,
                leaf_trigger_tick,
            },
        )
        .0
    }

    pub fn apply_block_edits_conditionally_journaled(
        &self,
        decision_id: u64,
        edits: &[ResidentBlockEdit],
        preconditions: &[ResidentBlockPrecondition],
        scheduled_block_ticks: &[ScheduledBlockTick],
        light_table: Option<&BlockLightTable>,
        leaf_trigger_tick: Option<u64>,
    ) -> (ResidentBlockEditBatchResult, Vec<ChunkPos>) {
        self.apply_block_edits_conditionally_inner(
            Some(decision_id),
            ResidentBlockEditPlan {
                edits,
                preconditions,
                scheduled_block_ticks,
                consumed_block_ticks: &[],
                consumed_fluid_ticks: &[],
                scheduled_fluid_ticks: &[],
                light_table,
                leaf_trigger_tick,
            },
        )
    }

    pub fn apply_fluid_tick_plan_conditionally(
        &self,
        plan: &ResidentFluidTickPlan<'_>,
    ) -> ResidentBlockEditBatchResult {
        self.apply_block_edits_conditionally_inner(
            None,
            ResidentBlockEditPlan {
                edits: plan.edits,
                preconditions: plan.preconditions,
                scheduled_block_ticks: &[],
                consumed_block_ticks: &[],
                consumed_fluid_ticks: plan.consumed_ticks,
                scheduled_fluid_ticks: plan.scheduled_ticks,
                light_table: plan.light_table,
                leaf_trigger_tick: plan.leaf_trigger_tick,
            },
        )
        .0
    }

    pub fn apply_fluid_tick_plan_conditionally_journaled(
        &self,
        decision_id: u64,
        plan: &ResidentFluidTickPlan<'_>,
    ) -> (ResidentBlockEditBatchResult, Vec<ChunkPos>) {
        self.apply_block_edits_conditionally_inner(
            Some(decision_id),
            ResidentBlockEditPlan {
                edits: plan.edits,
                preconditions: plan.preconditions,
                scheduled_block_ticks: &[],
                consumed_block_ticks: &[],
                consumed_fluid_ticks: plan.consumed_ticks,
                scheduled_fluid_ticks: plan.scheduled_ticks,
                light_table: plan.light_table,
                leaf_trigger_tick: plan.leaf_trigger_tick,
            },
        )
    }

    pub fn apply_scheduled_block_tick_plan_conditionally(
        &self,
        plan: &ResidentScheduledBlockTickPlan<'_>,
    ) -> ResidentBlockEditBatchResult {
        self.apply_block_edits_conditionally_inner(
            None,
            ResidentBlockEditPlan {
                edits: plan.edits,
                preconditions: plan.preconditions,
                scheduled_block_ticks: &[],
                consumed_block_ticks: plan.consumed_ticks,
                consumed_fluid_ticks: &[],
                scheduled_fluid_ticks: &[],
                light_table: plan.light_table,
                leaf_trigger_tick: plan.leaf_trigger_tick,
            },
        )
        .0
    }

    pub fn apply_scheduled_block_tick_plan_conditionally_journaled(
        &self,
        decision_id: u64,
        plan: &ResidentScheduledBlockTickPlan<'_>,
    ) -> (ResidentBlockEditBatchResult, Vec<ChunkPos>) {
        self.apply_block_edits_conditionally_inner(
            Some(decision_id),
            ResidentBlockEditPlan {
                edits: plan.edits,
                preconditions: plan.preconditions,
                scheduled_block_ticks: &[],
                consumed_block_ticks: plan.consumed_ticks,
                consumed_fluid_ticks: &[],
                scheduled_fluid_ticks: &[],
                light_table: plan.light_table,
                leaf_trigger_tick: plan.leaf_trigger_tick,
            },
        )
    }

    /// Prepare a cross-region scheduled block tick without mutating a resident
    /// store or publishing a world snapshot. Callers must persist
    /// [`ResidentCrossRegionScheduledBlockTickTransaction::journal_snapshots`]
    /// through
    /// [`ResidentCrossRegionScheduledBlockTickTransaction::commit_durably`].
    pub fn prepare_cross_region_scheduled_block_tick_transaction(
        &self,
        decision_id: Option<u64>,
        plan: &ResidentScheduledBlockTickPlan<'_>,
    ) -> ResidentCrossRegionScheduledBlockTickPrepareResult {
        let publication = self.resident.read_view.publication_state();
        let _mutation = publication.mutation();
        let mut required = HashSet::new();
        required.extend(plan.edits.iter().map(|edit| chunk_pos_of(edit.pos)));
        required.extend(
            plan.preconditions
                .iter()
                .map(|precondition| chunk_pos_of(precondition.pos)),
        );
        required.extend(
            plan.consumed_ticks
                .iter()
                .map(|tick| chunk_pos_of(tick.pos)),
        );

        let mut positions = required.iter().copied().collect::<Vec<_>>();
        if plan.leaf_trigger_tick.is_some() {
            for edit in plan.edits {
                let Some(neighbours) = block_neighbours(edit.pos) else {
                    return ResidentCrossRegionScheduledBlockTickPrepareResult::Stale;
                };
                positions.extend(neighbours.into_iter().map(chunk_pos_of));
            }
        }
        positions.sort_unstable_by_key(|position| (region_of(*position), position.x, position.z));
        positions.dedup();

        let mut region_positions = positions.iter().copied().map(region_of).collect::<Vec<_>>();
        region_positions.sort_unstable();
        region_positions.dedup();

        let mut chunks = Vec::with_capacity(positions.len());
        for region_position in region_positions {
            let region_chunks = positions
                .iter()
                .copied()
                .filter(|position| region_of(*position) == region_position)
                .collect::<Vec<_>>();
            let Some(region) = self.resident.region(region_chunks[0]) else {
                if region_chunks
                    .iter()
                    .any(|position| required.contains(position))
                {
                    return ResidentCrossRegionScheduledBlockTickPrepareResult::Missing;
                }
                chunks.extend(region_chunks.into_iter().map(|position| {
                    ResidentCrossRegionStagedChunk {
                        position,
                        expected: None,
                        staged: None,
                    }
                }));
                continue;
            };
            let region = region
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for position in region_chunks {
                let Some(chunk) = region.chunks.get(&position) else {
                    if required.contains(&position) {
                        return ResidentCrossRegionScheduledBlockTickPrepareResult::Missing;
                    }
                    chunks.push(ResidentCrossRegionStagedChunk {
                        position,
                        expected: None,
                        staged: None,
                    });
                    continue;
                };
                if region.pending_journal_lsn.contains_key(&position)
                    || decision_id
                        .is_some_and(|decision_id| chunk.world_journal_lsn() > decision_id)
                {
                    return ResidentCrossRegionScheduledBlockTickPrepareResult::Stale;
                }
                chunks.push(ResidentCrossRegionStagedChunk {
                    position,
                    expected: Some(Arc::clone(chunk)),
                    staged: Some(Arc::new((**chunk).clone())),
                });
            }
        }
        chunks.sort_unstable_by_key(|chunk| {
            (
                region_of(chunk.position),
                chunk.position.x,
                chunk.position.z,
            )
        });

        for precondition in plan.preconditions {
            let Some(chunk) = cross_region_staged_chunk(&chunks, chunk_pos_of(precondition.pos))
            else {
                return ResidentCrossRegionScheduledBlockTickPrepareResult::Missing;
            };
            let local_x = precondition.pos.x.rem_euclid(SECTION_DIM as i32) as u8;
            let local_z = precondition.pos.z.rem_euclid(SECTION_DIM as i32) as u8;
            let Some(staged) = chunk.staged.as_ref() else {
                return ResidentCrossRegionScheduledBlockTickPrepareResult::Missing;
            };
            if staged.get_block(local_x, precondition.pos.y, local_z)
                != Some(precondition.expected_state)
                || staged.block_mutation_token(local_x, precondition.pos.y, local_z)
                    != Some(precondition.expected_token)
            {
                return ResidentCrossRegionScheduledBlockTickPrepareResult::Stale;
            }
        }

        let mut consumed_by_chunk = HashMap::<ChunkPos, Vec<ScheduledBlockTick>>::new();
        for tick in plan.consumed_ticks {
            consumed_by_chunk
                .entry(chunk_pos_of(tick.pos))
                .or_default()
                .push(tick.clone());
        }
        if consumed_by_chunk.iter().any(|(position, expected)| {
            cross_region_staged_chunk(&chunks, *position)
                .and_then(|chunk| chunk.staged.as_ref())
                .is_none_or(|chunk| !chunk.scheduled_block_ticks().starts_with(expected))
        }) {
            return ResidentCrossRegionScheduledBlockTickPrepareResult::Stale;
        }

        let air = self
            .resident
            .registry
            .block(&Identifier::parse("minecraft:air").expect("static identifier"))
            .map(|block| block.default)
            .unwrap_or(BlockStateId(0));
        let registry = Arc::clone(&self.resident.registry);
        let mut touched = HashSet::new();
        let mut consumed_by_chunk = consumed_by_chunk.into_iter().collect::<Vec<_>>();
        consumed_by_chunk.sort_unstable_by_key(|(position, _)| (position.x, position.z));
        for (position, expected) in consumed_by_chunk {
            let chunk = cross_region_staged_chunk_mut(&mut chunks, position)
                .expect("preflighted consumed block-tick chunk");
            let chunk = Arc::make_mut(chunk.staged.as_mut().expect("required staged chunk"));
            assert!(chunk.drain_scheduled_block_tick_prefix(&expected));
            if let Some(decision_id) = decision_id {
                chunk.set_world_journal_lsn(decision_id);
            }
            touched.insert(position);
        }

        let mut applied = Vec::with_capacity(plan.edits.len());
        for edit in plan.edits {
            let position = chunk_pos_of(edit.pos);
            let chunk = cross_region_staged_chunk_mut(&mut chunks, position)
                .expect("preflighted resident chunk");
            let local_x = edit.pos.x.rem_euclid(SECTION_DIM as i32) as u8;
            let local_z = edit.pos.z.rem_euclid(SECTION_DIM as i32) as u8;
            let previous = chunk
                .staged
                .as_ref()
                .expect("required staged chunk")
                .get_block(local_x, edit.pos.y, local_z)
                .expect("preflighted block position");
            let preserve_light = edit.preserve_light
                || plan
                    .light_table
                    .is_some_and(|table| same_light_behaviour(table, previous, edit.new_state));
            let previous_light = if previous != edit.new_state && !preserve_light {
                ChunkLight::from_chunk(chunk.staged.as_ref().expect("required staged chunk"))
            } else {
                None
            };
            let changes_light = previous != edit.new_state && !preserve_light;
            let chunk = Arc::make_mut(chunk.staged.as_mut().expect("required staged chunk"));
            let previous = if preserve_light {
                chunk.set_block_and_update_preserving_light(
                    local_x,
                    edit.pos.y,
                    local_z,
                    edit.new_state,
                    air,
                )
            } else {
                chunk.set_block_and_update(local_x, edit.pos.y, local_z, edit.new_state, air)
            }
            .expect("preflighted block position");
            if previous != edit.new_state {
                prune_incompatible_block_entities(chunk, edit.pos, &registry, edit.new_state);
                if !preserve_light && let Some(light_table) = plan.light_table {
                    chunk.update_highest_opaque_column(local_x, local_z, light_table);
                }
                if let Some(decision_id) = decision_id {
                    chunk.set_world_journal_lsn(decision_id);
                }
                let resulting_token = chunk
                    .block_mutation_token(local_x, edit.pos.y, local_z)
                    .expect("mutated block token");
                touched.insert(position);
                applied.push(ResidentAppliedBlockEdit {
                    pos: edit.pos,
                    previous,
                    new_state: edit.new_state,
                    resulting_token,
                    previous_light,
                    changes_light,
                });
            }
        }

        if let Some(trigger_tick) = plan.leaf_trigger_tick {
            let mut leaves = Vec::new();
            for edit in &applied {
                for position in block_neighbours(edit.pos).expect("preflighted neighbours") {
                    let Some(chunk) = cross_region_staged_chunk(&chunks, chunk_pos_of(position))
                    else {
                        continue;
                    };
                    let local_x = position.x.rem_euclid(SECTION_DIM as i32) as u8;
                    let local_z = position.z.rem_euclid(SECTION_DIM as i32) as u8;
                    let Some(state_id) = chunk
                        .staged
                        .as_ref()
                        .and_then(|chunk| chunk.get_block(local_x, position.y, local_z))
                    else {
                        continue;
                    };
                    let Some(state) = self.resident.registry.by_id(state_id) else {
                        continue;
                    };
                    if state.block.id.path().ends_with("_leaves")
                        && !leaves
                            .iter()
                            .any(|(existing, _): &(BlockPos, Identifier)| *existing == position)
                    {
                        leaves.push((position, state.block.id.clone()));
                    }
                }
            }
            leaves.sort_by_key(|(position, _)| (position.x, position.y, position.z));
            for (position, block) in leaves {
                let chunk_position = chunk_pos_of(position);
                let chunk = cross_region_staged_chunk_mut(&mut chunks, chunk_position)
                    .expect("prepared resident leaf chunk");
                let chunk = Arc::make_mut(chunk.staged.as_mut().expect("present leaf chunk"));
                if chunk.schedule_block_tick(ScheduledBlockTick::new(
                    position,
                    block,
                    trigger_tick,
                    0,
                )) {
                    if let Some(decision_id) = decision_id {
                        chunk.set_world_journal_lsn(decision_id);
                    }
                    touched.insert(chunk_position);
                }
            }
        }

        let mut touched = touched.into_iter().collect::<Vec<_>>();
        touched.sort_unstable_by_key(|position| (position.x, position.z));
        ResidentCrossRegionScheduledBlockTickPrepareResult::Prepared(
            ResidentCrossRegionScheduledBlockTickTransaction {
                resident: self.resident.clone(),
                chunks,
                applied,
                touched,
                #[cfg(test)]
                publish_hook: None,
            },
        )
    }

    fn apply_block_edits_conditionally_inner(
        &self,
        decision_id: Option<u64>,
        plan: ResidentBlockEditPlan<'_>,
    ) -> (ResidentBlockEditBatchResult, Vec<ChunkPos>) {
        let publication = self.resident.read_view.publication_state();
        let _mutation = publication.mutation();
        let ResidentBlockEditPlan {
            edits,
            preconditions,
            scheduled_block_ticks,
            consumed_block_ticks,
            consumed_fluid_ticks,
            scheduled_fluid_ticks,
            light_table,
            leaf_trigger_tick,
        } = plan;
        let mut positions = edits
            .iter()
            .map(|edit| edit.pos)
            .chain(preconditions.iter().map(|precondition| precondition.pos))
            .chain(scheduled_block_ticks.iter().map(|tick| tick.pos))
            .chain(consumed_block_ticks.iter().map(|tick| tick.pos))
            .chain(consumed_fluid_ticks.iter().map(|tick| tick.pos))
            .chain(scheduled_fluid_ticks.iter().map(|tick| tick.pos));
        let Some(first) = positions.next() else {
            return (
                ResidentBlockEditBatchResult::Applied(Vec::new()),
                Vec::new(),
            );
        };
        let owner = region_of(chunk_pos_of(first));
        if positions.any(|position| region_of(chunk_pos_of(position)) != owner) {
            return (ResidentBlockEditBatchResult::CrossRegion, Vec::new());
        }
        if leaf_trigger_tick.is_some()
            && edits.iter().any(|edit| {
                block_neighbours(edit.pos).is_none_or(|neighbours| {
                    neighbours
                        .into_iter()
                        .any(|position| region_of(chunk_pos_of(position)) != owner)
                })
            })
        {
            return (ResidentBlockEditBatchResult::CrossRegion, Vec::new());
        }

        let Some(region) = self.resident.region(chunk_pos_of(first)) else {
            return (ResidentBlockEditBatchResult::Missing, Vec::new());
        };
        let mut region = region
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if edits
            .iter()
            .map(|edit| chunk_pos_of(edit.pos))
            .chain(
                preconditions
                    .iter()
                    .map(|precondition| chunk_pos_of(precondition.pos)),
            )
            .chain(
                scheduled_block_ticks
                    .iter()
                    .map(|tick| chunk_pos_of(tick.pos)),
            )
            .chain(
                consumed_block_ticks
                    .iter()
                    .map(|tick| chunk_pos_of(tick.pos)),
            )
            .chain(
                consumed_fluid_ticks
                    .iter()
                    .map(|tick| chunk_pos_of(tick.pos)),
            )
            .chain(
                scheduled_fluid_ticks
                    .iter()
                    .map(|tick| chunk_pos_of(tick.pos)),
            )
            .any(|position| !region.chunks.contains_key(&position))
        {
            return (ResidentBlockEditBatchResult::Missing, Vec::new());
        }
        for precondition in preconditions {
            let chunk = &region.chunks[&chunk_pos_of(precondition.pos)];
            let local_x = precondition.pos.x.rem_euclid(SECTION_DIM as i32) as u8;
            let local_z = precondition.pos.z.rem_euclid(SECTION_DIM as i32) as u8;
            if chunk.get_block(local_x, precondition.pos.y, local_z)
                != Some(precondition.expected_state)
                || chunk.block_mutation_token(local_x, precondition.pos.y, local_z)
                    != Some(precondition.expected_token)
            {
                return (ResidentBlockEditBatchResult::Stale, Vec::new());
            }
        }

        let mut consumed_blocks_by_chunk = HashMap::<ChunkPos, Vec<ScheduledBlockTick>>::new();
        for tick in consumed_block_ticks {
            consumed_blocks_by_chunk
                .entry(chunk_pos_of(tick.pos))
                .or_default()
                .push(tick.clone());
        }
        if consumed_blocks_by_chunk.iter().any(|(position, expected)| {
            !region.chunks[position]
                .scheduled_block_ticks()
                .starts_with(expected)
        }) {
            return (ResidentBlockEditBatchResult::Stale, Vec::new());
        }

        let mut consumed_by_chunk = HashMap::<ChunkPos, Vec<ScheduledFluidTick>>::new();
        for tick in consumed_fluid_ticks {
            consumed_by_chunk
                .entry(chunk_pos_of(tick.pos))
                .or_default()
                .push(tick.clone());
        }
        if consumed_by_chunk.iter().any(|(position, expected)| {
            !region.chunks[position]
                .scheduled_fluid_ticks()
                .starts_with(expected)
        }) {
            return (ResidentBlockEditBatchResult::Stale, Vec::new());
        }

        let air = self
            .resident
            .registry
            .block(&Identifier::parse("minecraft:air").expect("static identifier"))
            .map(|block| block.default)
            .unwrap_or(BlockStateId(0));
        let registry = Arc::clone(&self.resident.registry);
        let mut applied = Vec::with_capacity(edits.len());
        let mut touched = HashSet::new();
        let mut consumed_blocks_by_chunk = consumed_blocks_by_chunk.into_iter().collect::<Vec<_>>();
        consumed_blocks_by_chunk.sort_unstable_by_key(|(position, _)| (position.x, position.z));
        for (chunk_position, expected) in consumed_blocks_by_chunk {
            let chunk = region
                .chunks
                .get_mut(&chunk_position)
                .expect("preflighted consumed block-tick chunk");
            self.resident
                .read_view
                .update_chunk(chunk_position, chunk, |chunk| {
                    assert!(chunk.drain_scheduled_block_tick_prefix(&expected));
                    if let Some(decision_id) = decision_id {
                        chunk.set_world_journal_lsn(decision_id);
                    }
                });
            touched.insert(chunk_position);
            self.resident.scheduled_tick_view.publish_chunk(
                chunk_position,
                chunk,
                &self.resident.registry,
            );
        }
        let mut consumed_by_chunk = consumed_by_chunk.into_iter().collect::<Vec<_>>();
        consumed_by_chunk.sort_unstable_by_key(|(position, _)| (position.x, position.z));
        for (chunk_position, expected) in consumed_by_chunk {
            let chunk = region
                .chunks
                .get_mut(&chunk_position)
                .expect("preflighted consumed fluid-tick chunk");
            self.resident
                .read_view
                .update_chunk(chunk_position, chunk, |chunk| {
                    assert!(chunk.drain_scheduled_fluid_tick_prefix(&expected));
                    if let Some(decision_id) = decision_id {
                        chunk.set_world_journal_lsn(decision_id);
                    }
                });
            touched.insert(chunk_position);
            self.resident.scheduled_tick_view.publish_chunk(
                chunk_position,
                chunk,
                &self.resident.registry,
            );
        }
        for edit in edits {
            let chunk_position = chunk_pos_of(edit.pos);
            let chunk = region
                .chunks
                .get_mut(&chunk_position)
                .expect("preflighted resident chunk");
            let local_x = edit.pos.x.rem_euclid(SECTION_DIM as i32) as u8;
            let local_z = edit.pos.z.rem_euclid(SECTION_DIM as i32) as u8;
            let previous = chunk
                .get_block(local_x, edit.pos.y, local_z)
                .expect("preflighted block position");
            let preserve_light = edit.preserve_light
                || light_table
                    .is_some_and(|table| same_light_behaviour(table, previous, edit.new_state));
            let previous_light = if previous != edit.new_state && !preserve_light {
                ChunkLight::from_chunk(chunk)
            } else {
                None
            };
            let changes_light = previous != edit.new_state && !preserve_light;
            let mutation = self
                .resident
                .read_view
                .update_chunk(chunk_position, chunk, |chunk| {
                    let previous = if preserve_light {
                        chunk.set_block_and_update_preserving_light(
                            local_x,
                            edit.pos.y,
                            local_z,
                            edit.new_state,
                            air,
                        )
                    } else {
                        chunk.set_block_and_update(
                            local_x,
                            edit.pos.y,
                            local_z,
                            edit.new_state,
                            air,
                        )
                    }
                    .expect("preflighted block position");
                    if previous != edit.new_state {
                        prune_incompatible_block_entities(
                            chunk,
                            edit.pos,
                            &registry,
                            edit.new_state,
                        );
                        if !preserve_light && let Some(light_table) = light_table {
                            chunk.update_highest_opaque_column(local_x, local_z, light_table);
                        }
                        if let Some(decision_id) = decision_id {
                            chunk.set_world_journal_lsn(decision_id);
                        }
                    }
                    let resulting_token = chunk
                        .block_mutation_token(local_x, edit.pos.y, local_z)
                        .expect("mutated block token");
                    (previous, resulting_token)
                });
            self.resident
                .read_view
                .publish_furnaces(chunk_position, chunk);
            self.resident.scheduled_tick_view.publish_chunk(
                chunk_position,
                chunk,
                &self.resident.registry,
            );
            let (previous, resulting_token) = mutation;
            if previous != edit.new_state {
                touched.insert(chunk_position);
                applied.push(ResidentAppliedBlockEdit {
                    pos: edit.pos,
                    previous,
                    new_state: edit.new_state,
                    resulting_token,
                    previous_light,
                    changes_light,
                });
            }
        }

        for tick in scheduled_block_ticks {
            if !applied.iter().any(|edit| edit.pos == tick.pos) {
                continue;
            }
            let chunk_position = chunk_pos_of(tick.pos);
            let chunk = region
                .chunks
                .get_mut(&chunk_position)
                .expect("preflighted scheduled-tick chunk");
            let added = self
                .resident
                .read_view
                .update_chunk(chunk_position, chunk, |chunk| {
                    let added = chunk.schedule_block_tick(tick.clone());
                    if added && let Some(decision_id) = decision_id {
                        chunk.set_world_journal_lsn(decision_id);
                    }
                    added
                });
            if added {
                touched.insert(chunk_position);
            }
            self.resident.scheduled_tick_view.publish_chunk(
                chunk_position,
                chunk,
                &self.resident.registry,
            );
        }

        let mut scheduled_by_chunk = HashMap::<ChunkPos, Vec<ScheduledFluidTick>>::new();
        for tick in scheduled_fluid_ticks {
            scheduled_by_chunk
                .entry(chunk_pos_of(tick.pos))
                .or_default()
                .push(tick.clone());
        }
        let mut scheduled_by_chunk = scheduled_by_chunk.into_iter().collect::<Vec<_>>();
        scheduled_by_chunk.sort_unstable_by_key(|(position, _)| (position.x, position.z));
        for (chunk_position, ticks) in scheduled_by_chunk {
            let chunk = region
                .chunks
                .get_mut(&chunk_position)
                .expect("preflighted scheduled fluid-tick chunk");
            let added = self
                .resident
                .read_view
                .update_chunk(chunk_position, chunk, move |chunk| {
                    let added = chunk.schedule_fluid_tick_batch(ticks);
                    if added != 0
                        && let Some(decision_id) = decision_id
                    {
                        chunk.set_world_journal_lsn(decision_id);
                    }
                    added
                });
            if added != 0 {
                touched.insert(chunk_position);
            }
            self.resident.scheduled_tick_view.publish_chunk(
                chunk_position,
                chunk,
                &self.resident.registry,
            );
        }

        if let Some(trigger_tick) = leaf_trigger_tick {
            let mut leaves = Vec::new();
            for edit in &applied {
                for position in block_neighbours(edit.pos).expect("preflighted neighbours") {
                    let chunk_position = chunk_pos_of(position);
                    let Some(chunk) = region.chunks.get(&chunk_position) else {
                        continue;
                    };
                    let local_x = position.x.rem_euclid(SECTION_DIM as i32) as u8;
                    let local_z = position.z.rem_euclid(SECTION_DIM as i32) as u8;
                    let Some(state_id) = chunk.get_block(local_x, position.y, local_z) else {
                        continue;
                    };
                    let Some(state) = self.resident.registry.by_id(state_id) else {
                        continue;
                    };
                    if state.block.id.path().ends_with("_leaves")
                        && !leaves
                            .iter()
                            .any(|(existing, _): &(BlockPos, Identifier)| *existing == position)
                    {
                        leaves.push((position, state.block.id.clone()));
                    }
                }
            }
            leaves.sort_by_key(|(position, _)| (position.x, position.y, position.z));
            for (position, block) in leaves {
                let chunk_position = chunk_pos_of(position);
                let chunk = region
                    .chunks
                    .get_mut(&chunk_position)
                    .expect("resident leaf chunk");
                let added = self
                    .resident
                    .read_view
                    .update_chunk(chunk_position, chunk, |chunk| {
                        let added = chunk.schedule_block_tick(ScheduledBlockTick::new(
                            position,
                            block,
                            trigger_tick,
                            0,
                        ));
                        if added && let Some(decision_id) = decision_id {
                            chunk.set_world_journal_lsn(decision_id);
                        }
                        added
                    });
                if added {
                    touched.insert(chunk_position);
                }
                self.resident.scheduled_tick_view.publish_chunk(
                    chunk_position,
                    chunk,
                    &self.resident.registry,
                );
            }
        }

        let mut touched = touched.into_iter().collect::<Vec<_>>();
        touched.sort_unstable_by_key(|position| (position.x, position.z));
        if let Some(decision_id) = decision_id {
            for &position in &touched {
                region.pending_journal_lsn.insert(position, decision_id);
            }
        }
        (ResidentBlockEditBatchResult::Applied(applied), touched)
    }

    pub fn set_block_if_current(
        &self,
        position: BlockPos,
        expected_state: BlockStateId,
        expected_token: BlockMutationToken,
        state: BlockStateId,
        preserve_light: bool,
    ) -> ResidentBlockMutation {
        let chunk_position = chunk_pos_of(position);
        let local_x = position.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = position.z.rem_euclid(SECTION_DIM as i32) as u8;
        let air = self
            .resident
            .registry
            .block(&Identifier::parse("minecraft:air").expect("static identifier"))
            .map(|block| block.default)
            .unwrap_or(BlockStateId(0));
        let registry = Arc::clone(&self.resident.registry);
        self.resident
            .mutate(chunk_position, |chunk| {
                if chunk.get_block(local_x, position.y, local_z) != Some(expected_state)
                    || chunk.block_mutation_token(local_x, position.y, local_z)
                        != Some(expected_token)
                {
                    return ResidentBlockMutation::Stale;
                }
                let previous = if preserve_light {
                    chunk.set_block_and_update_preserving_light(
                        local_x, position.y, local_z, state, air,
                    )
                } else {
                    chunk.set_block_and_update(local_x, position.y, local_z, state, air)
                }
                .expect("validated block position remains in chunk");
                if previous != state {
                    prune_incompatible_block_entities(chunk, position, &registry, state);
                }
                ResidentBlockMutation::Applied(previous)
            })
            .unwrap_or(ResidentBlockMutation::Missing)
    }
}

impl ResidentCrossRegionScheduledBlockTickTransaction {
    #[cfg(test)]
    fn set_publish_hook(&mut self, hook: Arc<dyn Fn(ChunkPos) + Send + Sync>) {
        self.publish_hook = Some(hook);
    }
    #[must_use]
    pub fn touched_chunks(&self) -> &[ChunkPos] {
        &self.touched
    }

    #[must_use]
    pub fn journal_snapshots(&self) -> Vec<ChunkSnapshot> {
        self.touched
            .iter()
            .map(|position| {
                Arc::clone(
                    cross_region_staged_chunk(&self.chunks, *position)
                        .expect("touched cross-region chunk remains staged")
                        .staged
                        .as_ref()
                        .expect("touched cross-region chunk is present"),
                )
            })
            .collect()
    }

    /// Verifies sources, invokes `persist`, and publishes while holding
    /// exclusive resident admission. `persist` must not re-enter resident
    /// readers or mutators.
    pub fn commit_durably<E>(
        self,
        persist: impl FnOnce(Vec<ChunkSnapshot>) -> Result<(), E>,
    ) -> ResidentCrossRegionScheduledBlockTickCommitResult<E> {
        let publication = self.resident.read_view.publication_state();
        let transaction = publication.transaction();
        if let Some(result) = self.verify_sources() {
            return match result {
                ResidentBlockEditBatchResult::Missing => {
                    ResidentCrossRegionScheduledBlockTickCommitResult::Missing
                }
                ResidentBlockEditBatchResult::Stale => {
                    ResidentCrossRegionScheduledBlockTickCommitResult::Stale
                }
                ResidentBlockEditBatchResult::Applied(_)
                | ResidentBlockEditBatchResult::CrossRegion => unreachable!("verification result"),
            };
        }
        if let Err(error) = persist(self.journal_snapshots()) {
            return ResidentCrossRegionScheduledBlockTickCommitResult::DurabilityFailed(error);
        }
        let applied = self.publish(&publication, transaction);
        ResidentCrossRegionScheduledBlockTickCommitResult::Applied(applied)
    }

    fn verify_sources(&self) -> Option<ResidentBlockEditBatchResult> {
        for region_position in self.region_positions() {
            let region = self.region_for(region_position);
            let region = region.as_ref().map(|region| {
                region
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
            });
            for chunk in self.chunks_in_region(region_position) {
                let current = region
                    .as_ref()
                    .and_then(|region| region.chunks.get(&chunk.position));
                match (&chunk.expected, current) {
                    (Some(expected), Some(current)) if Arc::ptr_eq(expected, current) => {}
                    (Some(_), None) => return Some(ResidentBlockEditBatchResult::Missing),
                    (None, None) => {}
                    _ => return Some(ResidentBlockEditBatchResult::Stale),
                }
            }
        }
        None
    }

    fn publish(
        self,
        publication: &crate::storage::ResidentPublicationState,
        transaction: RwLockWriteGuard<'_, ()>,
    ) -> Vec<ResidentAppliedBlockEdit> {
        let publishing = publication.begin_publish(transaction);
        for region_position in self.region_positions() {
            if !self
                .chunks_in_region(region_position)
                .any(|chunk| self.touched.contains(&chunk.position))
            {
                continue;
            }
            let region = self
                .region_for(region_position)
                .expect("required transaction region remains resident");
            let mut region = region
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            for chunk in self.chunks_in_region(region_position) {
                if !self.touched.contains(&chunk.position) {
                    continue;
                }
                region.chunks.insert(
                    chunk.position,
                    Arc::clone(chunk.staged.as_ref().expect("touched chunk is staged")),
                );
                region.pending_journal_lsn.remove(&chunk.position);
            }
            #[cfg(test)]
            let installed = self
                .chunks_in_region(region_position)
                .find(|chunk| self.touched.contains(&chunk.position))
                .map(|chunk| chunk.position);
            drop(region);
            #[cfg(test)]
            if let (Some(hook), Some(position)) = (&self.publish_hook, installed) {
                hook(position);
            }
        }
        for position in &self.touched {
            let chunk = cross_region_staged_chunk(&self.chunks, *position)
                .and_then(|chunk| chunk.staged.as_ref())
                .expect("touched cross-region chunk remains staged");
            self.resident.publish(*position, chunk);
            self.resident.read_view.publish_furnaces(*position, chunk);
        }
        publishing.complete();
        self.applied
    }

    fn region_positions(&self) -> Vec<WorldRegionPos> {
        let mut positions = self
            .chunks
            .iter()
            .map(|chunk| region_of(chunk.position))
            .collect::<Vec<_>>();
        positions.sort_unstable();
        positions.dedup();
        positions
    }

    fn chunks_in_region(
        &self,
        region_position: WorldRegionPos,
    ) -> impl Iterator<Item = &ResidentCrossRegionStagedChunk> {
        self.chunks
            .iter()
            .filter(move |chunk| region_of(chunk.position) == region_position)
    }

    fn region_for(&self, region_position: WorldRegionPos) -> Option<Arc<RwLock<ResidentRegion>>> {
        self.chunks_in_region(region_position)
            .next()
            .and_then(|chunk| self.resident.region(chunk.position))
    }
}

fn cross_region_staged_chunk(
    chunks: &[ResidentCrossRegionStagedChunk],
    position: ChunkPos,
) -> Option<&ResidentCrossRegionStagedChunk> {
    chunks.iter().find(|chunk| chunk.position == position)
}

fn cross_region_staged_chunk_mut(
    chunks: &mut [ResidentCrossRegionStagedChunk],
    position: ChunkPos,
) -> Option<&mut ResidentCrossRegionStagedChunk> {
    chunks.iter_mut().find(|chunk| chunk.position == position)
}

fn same_light_behaviour(
    table: &BlockLightTable,
    previous: BlockStateId,
    next: BlockStateId,
) -> bool {
    table.emission(previous.0).unwrap_or(0) == table.emission(next.0).unwrap_or(0)
        && table.opacity(previous.0).unwrap_or(0) == table.opacity(next.0).unwrap_or(0)
        && table.propagates_sky(previous.0).unwrap_or(true)
            == table.propagates_sky(next.0).unwrap_or(true)
}

fn hopper_transfer_positions(plan: &ResidentHopperTransferPlan) -> Vec<BlockPos> {
    let mut positions = plan
        .expected_states
        .iter()
        .map(|(position, _)| *position)
        .chain(plan.hoppers.iter().map(|change| change.position))
        .chain(plan.chests.iter().map(|change| change.position))
        .chain(plan.furnaces.iter().map(|change| change.position))
        .chain(plan.scheduled_block_ticks.iter().map(|tick| tick.pos))
        .collect::<Vec<_>>();
    positions.sort_unstable_by_key(|position| (position.x, position.y, position.z));
    positions.dedup();
    positions
}

fn region_of(position: ChunkPos) -> WorldRegionPos {
    WorldRegionPos {
        x: position.x.div_euclid(WORLD_REGION_AXIS_CHUNKS),
        z: position.z.div_euclid(WORLD_REGION_AXIS_CHUNKS),
    }
}

fn chunk_pos_of(position: BlockPos) -> ChunkPos {
    ChunkPos {
        x: position.x.div_euclid(SECTION_DIM as i32),
        z: position.z.div_euclid(SECTION_DIM as i32),
    }
}

fn block_neighbours(position: BlockPos) -> Option<[BlockPos; 6]> {
    Some([
        BlockPos {
            x: position.x.checked_add(1)?,
            ..position
        },
        BlockPos {
            x: position.x.checked_sub(1)?,
            ..position
        },
        BlockPos {
            y: position.y.checked_add(1)?,
            ..position
        },
        BlockPos {
            y: position.y.checked_sub(1)?,
            ..position
        },
        BlockPos {
            z: position.z.checked_add(1)?,
            ..position
        },
        BlockPos {
            z: position.z.checked_sub(1)?,
            ..position
        },
    ])
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, mpsc};

    use mc_data::blocks::{BlockReport, BlockStateReport};

    use super::*;
    use crate::storage::WorldStorage;

    fn hopper_registry() -> Arc<BlockRegistry> {
        Arc::new(
            BlockRegistry::from_report(&[
                BlockReport {
                    id: Identifier::parse("minecraft:air").unwrap(),
                    properties: BTreeMap::new(),
                    states: vec![BlockStateReport {
                        id: 0,
                        default: true,
                        properties: BTreeMap::new(),
                    }],
                },
                BlockReport {
                    id: Identifier::parse("minecraft:stone").unwrap(),
                    properties: BTreeMap::new(),
                    states: vec![BlockStateReport {
                        id: 1,
                        default: true,
                        properties: BTreeMap::new(),
                    }],
                },
                BlockReport {
                    id: Identifier::parse("minecraft:hopper").unwrap(),
                    properties: BTreeMap::new(),
                    states: vec![BlockStateReport {
                        id: 2,
                        default: true,
                        properties: BTreeMap::new(),
                    }],
                },
            ])
            .unwrap(),
        )
    }

    struct CrossRegionHopperFixture {
        world: WorldStorage,
        hopper_position: BlockPos,
        chest_position: BlockPos,
        hopper_chunk: ChunkPos,
        chest_chunk: ChunkPos,
        due: ScheduledBlockTick,
        plan: ResidentHopperTransferPlan,
        initial_hopper: HopperBlockEntity,
        initial_chest: ChestBlockEntity,
        updated_hopper: HopperBlockEntity,
        updated_chest: ChestBlockEntity,
    }

    fn cross_region_hopper_fixture() -> CrossRegionHopperFixture {
        let mut world = WorldStorage::in_memory(hopper_registry());
        let hopper_position = BlockPos { x: 127, y: 2, z: 3 };
        let chest_position = BlockPos { x: 128, y: 2, z: 3 };
        let hopper_chunk = chunk_pos_of(hopper_position);
        let chest_chunk = chunk_pos_of(chest_position);
        let biome = Identifier::parse("minecraft:plains").unwrap();
        for chunk_position in [hopper_chunk, chest_chunk] {
            world
                .insert_generated_chunk(
                    chunk_position,
                    Chunk::empty(chunk_position, BlockStateId(0), biome.clone()),
                )
                .unwrap();
        }
        world
            .set_block_at(hopper_position, BlockStateId(2))
            .unwrap();
        world.set_block_at(chest_position, BlockStateId(1)).unwrap();

        let mut initial_hopper = HopperBlockEntity::default();
        initial_hopper.slots[0].item_id = 42;
        initial_hopper.slots[0].count = 1;
        world
            .set_hopper_block_entity(hopper_position, initial_hopper.clone())
            .unwrap();
        let initial_chest = ChestBlockEntity::default();
        world
            .set_chest_block_entity(chest_position, initial_chest.clone())
            .unwrap();

        let due = ScheduledBlockTick::new(
            hopper_position,
            Identifier::parse("minecraft:hopper").unwrap(),
            20,
            0,
        );
        world.schedule_block_tick(due.clone()).unwrap();

        let mut updated_hopper = initial_hopper.clone();
        updated_hopper.slots[0] = crate::FurnaceSlot::EMPTY;
        updated_hopper.transfer_cooldown = 8;
        let mut updated_chest = initial_chest.clone();
        updated_chest.slots[0].item_id = 42;
        updated_chest.slots[0].count = 1;
        let plan = ResidentHopperTransferPlan {
            expected_states: vec![
                (hopper_position, BlockStateId(2)),
                (chest_position, BlockStateId(1)),
            ],
            hoppers: vec![ResidentBlockEntityChange {
                position: hopper_position,
                expected: initial_hopper.clone(),
                updated: updated_hopper.clone(),
            }],
            chests: vec![ResidentBlockEntityChange {
                position: chest_position,
                expected: initial_chest.clone(),
                updated: updated_chest.clone(),
            }],
            furnaces: Vec::new(),
            scheduled_block_ticks: Vec::new(),
        };

        CrossRegionHopperFixture {
            world,
            hopper_position,
            chest_position,
            hopper_chunk,
            chest_chunk,
            due,
            plan,
            initial_hopper,
            initial_chest,
            updated_hopper,
            updated_chest,
        }
    }

    #[test]
    fn locked_light_recompute_publishes_one_current_result() {
        let mut world = WorldStorage::in_memory(hopper_registry());
        let position = ChunkPos { x: 0, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(position, Chunk::empty(position, BlockStateId(0), biome))
            .unwrap();
        let expected = ChunkLight::filled(7, 3);
        let mutation = world.mutation_view();
        let mut recomputes = 0;

        let updates = mutation.recompute_and_publish_baked_light(
            [position],
            |sources| {
                recomputes += 1;
                assert!(sources.get(&position).is_some_and(Option::is_some));
                vec![(position, expected.clone())]
            },
            |update| (update.0, &update.1),
        );

        assert_eq!(recomputes, 1);
        assert_eq!(updates, vec![(position, expected.clone())]);
        let current = world
            .cached_chunk_snapshot(position)
            .expect("published test chunk");
        assert_eq!(
            ChunkLight::from_section_lights(&current.section_lights),
            Some(expected)
        );
    }

    #[test]
    fn cross_region_hopper_commit_is_atomic_and_reports_both_chunks() {
        let mut fixture = cross_region_hopper_fixture();
        let mutation = fixture.world.mutation_view();

        let (result, touched) = mutation.commit_scheduled_hopper_transfer_conditionally_journaled(
            7,
            std::slice::from_ref(&fixture.due),
            &fixture.plan,
        );

        assert_eq!(result, ResidentHopperTransferCommitResult::Applied);
        assert_eq!(touched, vec![fixture.hopper_chunk, fixture.chest_chunk]);
        assert_eq!(
            fixture
                .world
                .hopper_block_entity(fixture.hopper_position)
                .unwrap(),
            Some(fixture.updated_hopper)
        );
        assert_eq!(
            fixture
                .world
                .chest_block_entity(fixture.chest_position)
                .unwrap(),
            Some(fixture.updated_chest)
        );
        assert!(
            fixture
                .world
                .scheduled_block_ticks(fixture.hopper_chunk)
                .unwrap()
                .unwrap()
                .is_empty()
        );
        for &chunk_position in &touched {
            assert_eq!(
                fixture
                    .world
                    .cached_chunk_snapshot(chunk_position)
                    .unwrap()
                    .world_journal_lsn(),
                7
            );
        }
        assert!(fixture.world.plan_dirty_flush().unwrap().is_empty());
        assert_eq!(mutation.clear_journal_pending_conditionally(7, &touched), 2);
    }

    #[test]
    fn cross_region_hopper_stale_endpoint_changes_nothing() {
        let mut fixture = cross_region_hopper_fixture();
        fixture.plan.chests[0].expected.slots[0].count = 99;
        let mutation = fixture.world.mutation_view();
        let scheduled_before = fixture
            .world
            .scheduled_block_ticks(fixture.hopper_chunk)
            .unwrap()
            .unwrap()
            .to_vec();

        let (result, touched) = mutation.commit_scheduled_hopper_transfer_conditionally_journaled(
            7,
            std::slice::from_ref(&fixture.due),
            &fixture.plan,
        );

        assert_eq!(result, ResidentHopperTransferCommitResult::Stale);
        assert!(touched.is_empty());
        assert_eq!(
            fixture
                .world
                .hopper_block_entity(fixture.hopper_position)
                .unwrap(),
            Some(fixture.initial_hopper)
        );
        assert_eq!(
            fixture
                .world
                .chest_block_entity(fixture.chest_position)
                .unwrap(),
            Some(fixture.initial_chest)
        );
        assert_eq!(
            fixture
                .world
                .scheduled_block_ticks(fixture.hopper_chunk)
                .unwrap()
                .unwrap(),
            scheduled_before
        );
    }

    fn cross_region_scheduled_block_fixture(
        include_east: bool,
    ) -> (WorldStorage, BlockPos, BlockPos, ScheduledBlockTick) {
        let mut world = WorldStorage::in_memory(hopper_registry());
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let west_chunk = ChunkPos { x: 7, z: 0 };
        world
            .insert_generated_chunk(
                west_chunk,
                Chunk::empty(west_chunk, BlockStateId(0), biome.clone()),
            )
            .unwrap();
        let west = BlockPos { x: 127, y: 2, z: 3 };
        let east = BlockPos { x: 128, y: 2, z: 3 };
        world.set_block_at(west, BlockStateId(1)).unwrap();
        if include_east {
            let east_chunk = ChunkPos { x: 8, z: 0 };
            world
                .insert_generated_chunk(
                    east_chunk,
                    Chunk::empty(east_chunk, BlockStateId(0), biome),
                )
                .unwrap();
            world.set_block_at(east, BlockStateId(1)).unwrap();
        }
        let due =
            ScheduledBlockTick::new(west, Identifier::parse("minecraft:stone").unwrap(), 20, 0);
        world.schedule_block_tick(due.clone()).unwrap();
        (world, west, east, due)
    }

    #[test]
    fn cross_region_scheduled_block_transaction_publishes_only_after_durability() {
        let (world, west, east, due) = cross_region_scheduled_block_fixture(true);
        let read = world.read_view();
        let mutation = world.mutation_view();
        let west_token = world.block_mutation_token(west).unwrap();
        let east_token = world.block_mutation_token(east).unwrap();
        let edits = [
            ResidentBlockEdit {
                pos: west,
                new_state: BlockStateId(0),
                preserve_light: true,
            },
            ResidentBlockEdit {
                pos: east,
                new_state: BlockStateId(0),
                preserve_light: true,
            },
        ];
        let preconditions = [
            ResidentBlockPrecondition {
                pos: west,
                expected_state: BlockStateId(1),
                expected_token: west_token,
            },
            ResidentBlockPrecondition {
                pos: east,
                expected_state: BlockStateId(1),
                expected_token: east_token,
            },
        ];
        let prepared = mutation.prepare_cross_region_scheduled_block_tick_transaction(
            Some(7),
            &ResidentScheduledBlockTickPlan {
                consumed_ticks: std::slice::from_ref(&due),
                edits: &edits,
                preconditions: &preconditions,
                light_table: None,
                leaf_trigger_tick: None,
            },
        );
        let ResidentCrossRegionScheduledBlockTickPrepareResult::Prepared(transaction) = prepared
        else {
            panic!("current owners prepare a cross-region scheduled block transaction");
        };

        assert_eq!(read.get_cached_block(west), Some(BlockStateId(1)));
        assert_eq!(read.get_cached_block(east), Some(BlockStateId(1)));
        assert_eq!(transaction.journal_snapshots().len(), 2);
        assert!(
            transaction
                .journal_snapshots()
                .iter()
                .all(|chunk| chunk.world_journal_lsn() == 7)
        );

        let result = transaction.commit_durably(|snapshots| {
            assert_eq!(snapshots.len(), 2);
            Ok::<_, ()>(())
        });
        let ResidentCrossRegionScheduledBlockTickCommitResult::Applied(applied) = result else {
            panic!("durable current transaction applies");
        };
        assert_eq!(applied.len(), 2);
        assert_eq!(read.get_cached_block(west), Some(BlockStateId(0)));
        assert_eq!(read.get_cached_block(east), Some(BlockStateId(0)));
    }

    #[test]
    fn cross_region_reader_cannot_return_between_owner_installs() {
        let (world, west, east, due) = cross_region_scheduled_block_fixture(true);
        let read = world.read_view();
        let mutation = world.mutation_view();
        let prepared = mutation.prepare_cross_region_scheduled_block_tick_transaction(
            Some(7),
            &ResidentScheduledBlockTickPlan {
                consumed_ticks: std::slice::from_ref(&due),
                edits: &[
                    ResidentBlockEdit {
                        pos: west,
                        new_state: BlockStateId(0),
                        preserve_light: true,
                    },
                    ResidentBlockEdit {
                        pos: east,
                        new_state: BlockStateId(0),
                        preserve_light: true,
                    },
                ],
                preconditions: &[],
                light_table: None,
                leaf_trigger_tick: None,
            },
        );
        let ResidentCrossRegionScheduledBlockTickPrepareResult::Prepared(mut transaction) =
            prepared
        else {
            panic!("current owners prepare a cross-region scheduled block transaction");
        };
        let first = Arc::new(AtomicBool::new(true));
        let (installed_tx, installed_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        transaction.set_publish_hook(Arc::new(move |_| {
            if first.swap(false, Ordering::AcqRel) {
                installed_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
            }
        }));
        let publish = std::thread::spawn(move || transaction.commit_durably(|_| Ok::<_, ()>(())));
        installed_rx.recv().unwrap();

        let (reader_started_tx, reader_started_rx) = mpsc::channel();
        let (reader_done_tx, reader_done_rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            reader_started_tx.send(()).unwrap();
            let snapshots = read.snapshot_chunks(&[chunk_pos_of(west), chunk_pos_of(east)]);
            reader_done_tx
                .send((
                    snapshots.get_cached_block(west),
                    snapshots.get_cached_block(east),
                ))
                .unwrap();
        });
        reader_started_rx.recv().unwrap();
        assert!(matches!(
            reader_done_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        release_tx.send(()).unwrap();
        assert_eq!(
            reader_done_rx.recv().unwrap(),
            (Some(BlockStateId(0)), Some(BlockStateId(0)))
        );
        reader.join().unwrap();
        assert!(matches!(
            publish.join().unwrap(),
            ResidentCrossRegionScheduledBlockTickCommitResult::Applied(_)
        ));
    }

    #[test]
    fn cross_region_mutation_waits_until_all_owner_installs_finish() {
        let (world, west, east, due) = cross_region_scheduled_block_fixture(true);
        let mutation = world.mutation_view();
        let concurrent = mutation.clone();
        let west_token = world.block_mutation_token(west).unwrap();
        let prepared = mutation.prepare_cross_region_scheduled_block_tick_transaction(
            Some(7),
            &ResidentScheduledBlockTickPlan {
                consumed_ticks: std::slice::from_ref(&due),
                edits: &[
                    ResidentBlockEdit {
                        pos: west,
                        new_state: BlockStateId(0),
                        preserve_light: true,
                    },
                    ResidentBlockEdit {
                        pos: east,
                        new_state: BlockStateId(0),
                        preserve_light: true,
                    },
                ],
                preconditions: &[],
                light_table: None,
                leaf_trigger_tick: None,
            },
        );
        let ResidentCrossRegionScheduledBlockTickPrepareResult::Prepared(mut transaction) =
            prepared
        else {
            panic!("current owners prepare a cross-region scheduled block transaction");
        };
        let first = Arc::new(AtomicBool::new(true));
        let (installed_tx, installed_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        transaction.set_publish_hook(Arc::new(move |_| {
            if first.swap(false, Ordering::AcqRel) {
                installed_tx.send(()).unwrap();
                release_rx.lock().unwrap().recv().unwrap();
            }
        }));
        let publish = std::thread::spawn(move || transaction.commit_durably(|_| Ok::<_, ()>(())));
        installed_rx.recv().unwrap();

        let (mutation_started_tx, mutation_started_rx) = mpsc::channel();
        let (mutation_done_tx, mutation_done_rx) = mpsc::channel();
        let mutator = std::thread::spawn(move || {
            mutation_started_tx.send(()).unwrap();
            mutation_done_tx
                .send(concurrent.set_block_if_current(
                    west,
                    BlockStateId(1),
                    west_token,
                    BlockStateId(1),
                    true,
                ))
                .unwrap();
        });
        mutation_started_rx.recv().unwrap();
        assert!(matches!(
            mutation_done_rx.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));

        release_tx.send(()).unwrap();
        assert_eq!(
            mutation_done_rx.recv().unwrap(),
            ResidentBlockMutation::Stale
        );
        mutator.join().unwrap();
        assert!(matches!(
            publish.join().unwrap(),
            ResidentCrossRegionScheduledBlockTickCommitResult::Applied(_)
        ));
    }

    #[test]
    fn cancelled_cross_region_scheduled_block_transaction_publishes_nothing() {
        let (mut world, west, east, due) = cross_region_scheduled_block_fixture(true);
        let read = world.read_view();
        let mutation = world.mutation_view();
        let west_token = world.block_mutation_token(west).unwrap();
        let east_token = world.block_mutation_token(east).unwrap();
        let prepared = mutation.prepare_cross_region_scheduled_block_tick_transaction(
            Some(7),
            &ResidentScheduledBlockTickPlan {
                consumed_ticks: std::slice::from_ref(&due),
                edits: &[
                    ResidentBlockEdit {
                        pos: west,
                        new_state: BlockStateId(0),
                        preserve_light: true,
                    },
                    ResidentBlockEdit {
                        pos: east,
                        new_state: BlockStateId(0),
                        preserve_light: true,
                    },
                ],
                preconditions: &[
                    ResidentBlockPrecondition {
                        pos: west,
                        expected_state: BlockStateId(1),
                        expected_token: west_token,
                    },
                    ResidentBlockPrecondition {
                        pos: east,
                        expected_state: BlockStateId(1),
                        expected_token: east_token,
                    },
                ],
                light_table: None,
                leaf_trigger_tick: None,
            },
        );
        let ResidentCrossRegionScheduledBlockTickPrepareResult::Prepared(transaction) = prepared
        else {
            panic!("current owners prepare a cross-region scheduled block transaction");
        };

        drop(transaction);
        assert_eq!(read.get_cached_block(west), Some(BlockStateId(1)));
        assert_eq!(read.get_cached_block(east), Some(BlockStateId(1)));
        assert_eq!(
            world
                .scheduled_block_ticks(chunk_pos_of(west))
                .unwrap()
                .unwrap(),
            std::slice::from_ref(&due)
        );
    }

    #[test]
    fn dropped_prepared_cross_region_transaction_installs_nothing() {
        let (mut world, west, east, due) = cross_region_scheduled_block_fixture(true);
        let read = world.read_view();
        let mutation = world.mutation_view();
        let west_token = world.block_mutation_token(west).unwrap();
        let east_token = world.block_mutation_token(east).unwrap();
        let prepared = mutation.prepare_cross_region_scheduled_block_tick_transaction(
            Some(7),
            &ResidentScheduledBlockTickPlan {
                consumed_ticks: std::slice::from_ref(&due),
                edits: &[
                    ResidentBlockEdit {
                        pos: west,
                        new_state: BlockStateId(0),
                        preserve_light: true,
                    },
                    ResidentBlockEdit {
                        pos: east,
                        new_state: BlockStateId(0),
                        preserve_light: true,
                    },
                ],
                preconditions: &[
                    ResidentBlockPrecondition {
                        pos: west,
                        expected_state: BlockStateId(1),
                        expected_token: west_token,
                    },
                    ResidentBlockPrecondition {
                        pos: east,
                        expected_state: BlockStateId(1),
                        expected_token: east_token,
                    },
                ],
                light_table: None,
                leaf_trigger_tick: None,
            },
        );
        let ResidentCrossRegionScheduledBlockTickPrepareResult::Prepared(transaction) = prepared
        else {
            panic!("current owners prepare a cross-region scheduled block transaction");
        };

        drop(transaction);

        assert_eq!(read.get_cached_block(west), Some(BlockStateId(1)));
        assert_eq!(read.get_cached_block(east), Some(BlockStateId(1)));
        assert_eq!(world.get_cached_block(west), Some(BlockStateId(1)));
        assert_eq!(world.get_cached_block(east), Some(BlockStateId(1)));
        assert_eq!(
            world
                .scheduled_block_ticks(chunk_pos_of(west))
                .unwrap()
                .unwrap(),
            std::slice::from_ref(&due)
        );
    }

    #[test]
    fn cross_region_durability_failure_installs_nothing() {
        let (mut world, west, east, due) = cross_region_scheduled_block_fixture(true);
        let read = world.read_view();
        let mutation = world.mutation_view();
        let prepared = mutation.prepare_cross_region_scheduled_block_tick_transaction(
            Some(7),
            &ResidentScheduledBlockTickPlan {
                consumed_ticks: std::slice::from_ref(&due),
                edits: &[
                    ResidentBlockEdit {
                        pos: west,
                        new_state: BlockStateId(0),
                        preserve_light: true,
                    },
                    ResidentBlockEdit {
                        pos: east,
                        new_state: BlockStateId(0),
                        preserve_light: true,
                    },
                ],
                preconditions: &[],
                light_table: None,
                leaf_trigger_tick: None,
            },
        );
        let ResidentCrossRegionScheduledBlockTickPrepareResult::Prepared(transaction) = prepared
        else {
            panic!("current owners prepare a cross-region scheduled block transaction");
        };

        assert!(matches!(
            transaction.commit_durably(|_| Err("injected append failure")),
            ResidentCrossRegionScheduledBlockTickCommitResult::DurabilityFailed(
                "injected append failure"
            )
        ));
        assert_eq!(read.get_cached_block(west), Some(BlockStateId(1)));
        assert_eq!(read.get_cached_block(east), Some(BlockStateId(1)));
        assert_eq!(world.get_cached_block(west), Some(BlockStateId(1)));
        assert_eq!(world.get_cached_block(east), Some(BlockStateId(1)));
        assert_eq!(
            world
                .scheduled_block_ticks(chunk_pos_of(west))
                .unwrap()
                .unwrap(),
            std::slice::from_ref(&due)
        );
    }

    #[test]
    fn cross_region_prepare_rejects_absent_optional_neighbor_becoming_present() {
        let (mut world, west, _east, due) = cross_region_scheduled_block_fixture(false);
        let mutation = world.mutation_view();
        let west_token = world.block_mutation_token(west).unwrap();
        let prepared = mutation.prepare_cross_region_scheduled_block_tick_transaction(
            Some(7),
            &ResidentScheduledBlockTickPlan {
                consumed_ticks: std::slice::from_ref(&due),
                edits: &[ResidentBlockEdit {
                    pos: west,
                    new_state: BlockStateId(0),
                    preserve_light: true,
                }],
                preconditions: &[ResidentBlockPrecondition {
                    pos: west,
                    expected_state: BlockStateId(1),
                    expected_token: west_token,
                }],
                light_table: None,
                leaf_trigger_tick: Some(21),
            },
        );
        let ResidentCrossRegionScheduledBlockTickPrepareResult::Prepared(transaction) = prepared
        else {
            panic!("an absent optional neighbour remains a valid prepared source");
        };
        let east_chunk = ChunkPos { x: 8, z: 0 };
        world
            .insert_generated_chunk(
                east_chunk,
                Chunk::empty(
                    east_chunk,
                    BlockStateId(0),
                    Identifier::parse("minecraft:plains").unwrap(),
                ),
            )
            .unwrap();

        assert!(matches!(
            transaction.commit_durably(|_| Ok::<_, ()>(())),
            ResidentCrossRegionScheduledBlockTickCommitResult::Stale
        ));
        assert_eq!(world.get_cached_block(west), Some(BlockStateId(1)));
        assert_eq!(
            world
                .scheduled_block_ticks(chunk_pos_of(west))
                .unwrap()
                .unwrap(),
            std::slice::from_ref(&due)
        );
    }

    #[test]
    fn cross_region_scheduled_block_prepare_rejects_stale_second_owner_without_publication() {
        let (mut world, west, east, due) = cross_region_scheduled_block_fixture(true);
        let read = world.read_view();
        let mutation = world.mutation_view();
        let west_token = world.block_mutation_token(west).unwrap();
        let east_token = world.block_mutation_token(east).unwrap();
        let result = mutation.prepare_cross_region_scheduled_block_tick_transaction(
            Some(7),
            &ResidentScheduledBlockTickPlan {
                consumed_ticks: std::slice::from_ref(&due),
                edits: &[
                    ResidentBlockEdit {
                        pos: west,
                        new_state: BlockStateId(0),
                        preserve_light: true,
                    },
                    ResidentBlockEdit {
                        pos: east,
                        new_state: BlockStateId(0),
                        preserve_light: true,
                    },
                ],
                preconditions: &[
                    ResidentBlockPrecondition {
                        pos: west,
                        expected_state: BlockStateId(1),
                        expected_token: west_token,
                    },
                    ResidentBlockPrecondition {
                        pos: east,
                        expected_state: BlockStateId(0),
                        expected_token: east_token,
                    },
                ],
                light_table: None,
                leaf_trigger_tick: None,
            },
        );

        assert!(matches!(
            result,
            ResidentCrossRegionScheduledBlockTickPrepareResult::Stale
        ));
        assert_eq!(read.get_cached_block(west), Some(BlockStateId(1)));
        assert_eq!(read.get_cached_block(east), Some(BlockStateId(1)));
        assert_eq!(
            world
                .scheduled_block_ticks(chunk_pos_of(west))
                .unwrap()
                .unwrap(),
            std::slice::from_ref(&due)
        );
    }

    #[test]
    fn cross_region_scheduled_block_commit_rejects_second_owner_changed_after_prepare() {
        let (mut world, west, east, due) = cross_region_scheduled_block_fixture(true);
        let read = world.read_view();
        let mutation = world.mutation_view();
        let west_token = world.block_mutation_token(west).unwrap();
        let east_token = world.block_mutation_token(east).unwrap();
        let prepared = mutation.prepare_cross_region_scheduled_block_tick_transaction(
            Some(7),
            &ResidentScheduledBlockTickPlan {
                consumed_ticks: std::slice::from_ref(&due),
                edits: &[
                    ResidentBlockEdit {
                        pos: west,
                        new_state: BlockStateId(0),
                        preserve_light: true,
                    },
                    ResidentBlockEdit {
                        pos: east,
                        new_state: BlockStateId(0),
                        preserve_light: true,
                    },
                ],
                preconditions: &[
                    ResidentBlockPrecondition {
                        pos: west,
                        expected_state: BlockStateId(1),
                        expected_token: west_token,
                    },
                    ResidentBlockPrecondition {
                        pos: east,
                        expected_state: BlockStateId(1),
                        expected_token: east_token,
                    },
                ],
                light_table: None,
                leaf_trigger_tick: None,
            },
        );
        let ResidentCrossRegionScheduledBlockTickPrepareResult::Prepared(transaction) = prepared
        else {
            panic!("current owners prepare a cross-region scheduled block transaction");
        };

        world.set_block_at(east, BlockStateId(0)).unwrap();

        assert!(matches!(
            transaction.commit_durably(|_| Ok::<_, ()>(())),
            ResidentCrossRegionScheduledBlockTickCommitResult::Stale
        ));
        assert_eq!(read.get_cached_block(west), Some(BlockStateId(1)));
        assert_eq!(read.get_cached_block(east), Some(BlockStateId(0)));
        assert_eq!(
            world
                .scheduled_block_ticks(chunk_pos_of(west))
                .unwrap()
                .unwrap(),
            std::slice::from_ref(&due)
        );
    }

    #[test]
    fn cross_region_scheduled_block_prepare_rejects_missing_second_owner_without_publication() {
        let (mut world, west, east, due) = cross_region_scheduled_block_fixture(false);
        let read = world.read_view();
        let mutation = world.mutation_view();
        let west_token = world.block_mutation_token(west).unwrap();
        let result = mutation.prepare_cross_region_scheduled_block_tick_transaction(
            Some(7),
            &ResidentScheduledBlockTickPlan {
                consumed_ticks: std::slice::from_ref(&due),
                edits: &[
                    ResidentBlockEdit {
                        pos: west,
                        new_state: BlockStateId(0),
                        preserve_light: true,
                    },
                    ResidentBlockEdit {
                        pos: east,
                        new_state: BlockStateId(0),
                        preserve_light: true,
                    },
                ],
                preconditions: &[ResidentBlockPrecondition {
                    pos: west,
                    expected_state: BlockStateId(1),
                    expected_token: west_token,
                }],
                light_table: None,
                leaf_trigger_tick: None,
            },
        );

        assert!(matches!(
            result,
            ResidentCrossRegionScheduledBlockTickPrepareResult::Missing
        ));
        assert_eq!(read.get_cached_block(west), Some(BlockStateId(1)));
        assert_eq!(read.get_cached_block(east), None);
        assert_eq!(
            world
                .scheduled_block_ticks(chunk_pos_of(west))
                .unwrap()
                .unwrap(),
            std::slice::from_ref(&due)
        );
    }
}
