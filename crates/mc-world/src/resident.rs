use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

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
        let region = self.region(position)?;
        region
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .chunks
            .get(&position)
            .map(Arc::clone)
    }

    #[must_use]
    pub(crate) fn snapshots(&self) -> Vec<(ChunkPos, ChunkSnapshot)> {
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
    }

    #[must_use]
    pub(crate) fn flushable_snapshots(&self) -> Vec<(ChunkPos, ChunkSnapshot)> {
        let regions: Vec<_> = self
            .regions
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .values()
            .cloned()
            .collect();
        let mut snapshots = Vec::new();
        for region in regions {
            let region = region
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            snapshots.extend(
                region
                    .chunks
                    .iter()
                    .filter(|(position, _)| !region.pending_journal_lsn.contains_key(position))
                    .map(|(&position, chunk)| (position, Arc::clone(chunk))),
            );
        }
        snapshots
    }

    #[must_use]
    pub(crate) fn has_flushable_dirty(&self) -> bool {
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
    }

    pub(crate) fn stamp_world_journal_conditionally(
        &self,
        decision_id: u64,
        positions: &[ChunkPos],
    ) -> JournalStampResult {
        let mut positions = positions.to_vec();
        positions.sort_unstable_by_key(|position| (position.x, position.z));
        positions.dedup();
        let mut region_positions = positions.iter().copied().map(region_of).collect::<Vec<_>>();
        region_positions.sort_unstable();
        region_positions.dedup();

        let regions = self
            .regions
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut locked_regions = Vec::with_capacity(region_positions.len());
        for region_position in region_positions {
            let Some(region) = regions.get(&region_position) else {
                return JournalStampResult::Missing;
            };
            locked_regions.push((
                region_position,
                region
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            ));
        }

        let mut newer_decision = decision_id;
        for position in &positions {
            let Some((_, region)) = locked_regions
                .iter()
                .find(|(region, _)| *region == region_of(*position))
            else {
                return JournalStampResult::Missing;
            };
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
        for position in positions {
            let (_, region) = locked_regions
                .iter_mut()
                .find(|(region, _)| *region == region_of(position))
                .expect("preflighted journal region");
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
        JournalStampResult::Stamped(snapshots)
    }

    pub(crate) fn insert_if_absent(&self, position: ChunkPos, chunk: Chunk) -> bool {
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

    pub(crate) fn mutate_snapshot<R>(
        &self,
        position: ChunkPos,
        update: impl FnOnce(&mut ChunkSnapshot) -> R,
    ) -> Option<R> {
        let region = self.region(position)?;
        let mut region = region
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let chunk = region.chunks.get_mut(&position)?;
        let result = self
            .read_view
            .update_chunk_snapshot(position, chunk, update);
        self.read_view.publish_furnaces(position, chunk);
        self.scheduled_tick_view
            .publish_chunk(position, chunk, &self.registry);
        Some(result)
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
        let updates = updates.into_iter().collect::<Vec<_>>();
        let mut region_positions = expected_sources
            .keys()
            .copied()
            .chain(updates.iter().map(|(position, _)| *position))
            .map(region_of)
            .collect::<Vec<_>>();
        region_positions.sort_unstable();
        region_positions.dedup();

        let regions = self
            .resident
            .regions
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut locked_regions = Vec::with_capacity(region_positions.len());
        for position in region_positions {
            if let Some(region) = regions.get(&position) {
                locked_regions.push((
                    position,
                    region
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()),
                ));
            }
        }

        for (position, expected) in expected_sources {
            let current = locked_regions
                .iter()
                .find(|(region, _)| *region == region_of(*position))
                .and_then(|(_, region)| region.chunks.get(position));
            let is_current = match (expected, current) {
                (Some(expected), Some(current)) => Arc::ptr_eq(expected, current),
                (None, None) => true,
                _ => false,
            };
            if !is_current {
                return false;
            }
        }
        if updates.iter().any(|(position, _)| {
            locked_regions
                .iter()
                .find(|(region, _)| *region == region_of(*position))
                .is_none_or(|(_, region)| !region.chunks.contains_key(position))
        }) {
            return false;
        }

        for (position, light) in updates {
            let (_, region) = locked_regions
                .iter_mut()
                .find(|(region, _)| *region == region_of(position))
                .expect("preflighted baked-light region");
            let chunk = region
                .chunks
                .get_mut(&position)
                .expect("preflighted baked-light chunk");
            self.resident
                .read_view
                .update_chunk(position, chunk, |chunk| chunk.set_baked_light(light));
        }
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
        let mut source_positions = source_positions.into_iter().collect::<Vec<_>>();
        source_positions.sort_unstable_by_key(|position| (position.x, position.z));
        source_positions.dedup();
        let mut region_positions = source_positions
            .iter()
            .copied()
            .map(region_of)
            .collect::<Vec<_>>();
        region_positions.sort_unstable();
        region_positions.dedup();

        let regions = self
            .resident
            .regions
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut locked_regions = Vec::with_capacity(region_positions.len());
        for position in region_positions {
            if let Some(region) = regions.get(&position) {
                locked_regions.push((
                    position,
                    region
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner()),
                ));
            }
        }

        let sources = source_positions
            .into_iter()
            .map(|position| {
                let chunk = locked_regions
                    .iter()
                    .find(|(region, _)| *region == region_of(position))
                    .and_then(|(_, region)| region.chunks.get(&position))
                    .map(Arc::clone);
                (position, chunk)
            })
            .collect::<HashMap<_, _>>();
        let updates = recompute(&sources);
        let mut published = Vec::with_capacity(updates.len());
        for update in updates {
            let (position, light) = light_of(&update);
            let Some((_, region)) = locked_regions
                .iter_mut()
                .find(|(region, _)| *region == region_of(position))
            else {
                continue;
            };
            let Some(chunk) = region.chunks.get_mut(&position) else {
                continue;
            };
            self.resident
                .read_view
                .update_chunk(position, chunk, |chunk| chunk.set_baked_light(light));
            published.push(update);
        }
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
        let chunk_position = chunk_pos_of(position);
        let region = self.resident.region(chunk_position)?;
        let region = region
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let chunk = region.chunks.get(&chunk_position)?;
        let local_x = position.x.rem_euclid(SECTION_DIM as i32) as u8;
        let local_z = position.z.rem_euclid(SECTION_DIM as i32) as u8;
        Some((
            chunk.get_block(local_x, position.y, local_z)?,
            chunk.furnaces.get(&position).cloned().unwrap_or_default(),
        ))
    }

    pub fn backfill_hopper_ticks(&self, positions: &[ChunkPos], trigger_tick: u64) -> usize {
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
        expected: &FurnaceBlockEntity,
        updated: &FurnaceBlockEntity,
    ) -> ResidentFurnaceTickCommitResult {
        self.commit_furnace_tick_conditionally_inner(
            None,
            position,
            expected_state,
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
        expected: &FurnaceBlockEntity,
        updated: &FurnaceBlockEntity,
    ) -> (ResidentFurnaceTickCommitResult, Vec<ChunkPos>) {
        self.commit_furnace_tick_conditionally_inner(
            Some(decision_id),
            position,
            expected_state,
            expected,
            updated,
        )
    }

    fn commit_furnace_tick_conditionally_inner(
        &self,
        decision_id: Option<u64>,
        position: BlockPos,
        expected_state: BlockStateId,
        expected: &FurnaceBlockEntity,
        updated: &FurnaceBlockEntity,
    ) -> (ResidentFurnaceTickCommitResult, Vec<ChunkPos>) {
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
        let changed = self
            .resident
            .read_view
            .update_chunk(chunk_position, chunk, |chunk| {
                if chunk.furnaces.get(&position) != Some(updated) {
                    chunk.furnaces.insert(position, updated.clone());
                    chunk.mark_dirty();
                    if let Some(decision_id) = decision_id {
                        chunk.set_world_journal_lsn(decision_id);
                    }
                    true
                } else {
                    false
                }
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
        let mut positions = hopper_transfer_positions(plan);
        positions.extend(consumed_ticks.iter().map(|tick| tick.pos));
        positions.sort_unstable_by_key(|position| (position.x, position.y, position.z));
        positions.dedup();
        if positions.is_empty() {
            return (ResidentHopperTransferCommitResult::Missing, Vec::new());
        }
        let mut region_positions = positions
            .iter()
            .map(|position| region_of(chunk_pos_of(*position)))
            .collect::<Vec<_>>();
        region_positions.sort_unstable();
        region_positions.dedup();
        let regions = self
            .resident
            .regions
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut locked_regions = Vec::with_capacity(region_positions.len());
        for region_position in region_positions {
            let Some(region) = regions.get(&region_position) else {
                return (ResidentHopperTransferCommitResult::Missing, Vec::new());
            };
            locked_regions.push((
                region_position,
                region
                    .write()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            ));
        }
        if positions.iter().any(|position| {
            let chunk_position = chunk_pos_of(*position);
            locked_regions
                .iter()
                .find(|(region_position, _)| *region_position == region_of(chunk_position))
                .is_none_or(|(_, region)| !region.chunks.contains_key(&chunk_position))
        }) {
            return (ResidentHopperTransferCommitResult::Missing, Vec::new());
        }
        for (position, expected) in &plan.expected_states {
            let chunk_position = chunk_pos_of(*position);
            let (_, region) = locked_regions
                .iter()
                .find(|(region_position, _)| *region_position == region_of(chunk_position))
                .expect("preflighted hopper region");
            let chunk = &region.chunks[&chunk_position];
            let local_x = position.x.rem_euclid(SECTION_DIM as i32) as u8;
            let local_z = position.z.rem_euclid(SECTION_DIM as i32) as u8;
            if chunk.get_block(local_x, position.y, local_z) != Some(*expected) {
                return (ResidentHopperTransferCommitResult::Stale, Vec::new());
            }
        }
        if plan.hoppers.iter().any(|change| {
            let chunk_position = chunk_pos_of(change.position);
            let (_, region) = locked_regions
                .iter()
                .find(|(region_position, _)| *region_position == region_of(chunk_position))
                .expect("preflighted hopper region");
            region.chunks[&chunk_position]
                .hoppers
                .get(&change.position)
                .cloned()
                .unwrap_or_default()
                != change.expected
        }) || plan.chests.iter().any(|change| {
            let chunk_position = chunk_pos_of(change.position);
            let (_, region) = locked_regions
                .iter()
                .find(|(region_position, _)| *region_position == region_of(chunk_position))
                .expect("preflighted chest region");
            region.chunks[&chunk_position]
                .chests
                .get(&change.position)
                .cloned()
                .unwrap_or_default()
                != change.expected
        }) || plan.furnaces.iter().any(|change| {
            let chunk_position = chunk_pos_of(change.position);
            let (_, region) = locked_regions
                .iter()
                .find(|(region_position, _)| *region_position == region_of(chunk_position))
                .expect("preflighted furnace region");
            region.chunks[&chunk_position]
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
            let (_, region) = locked_regions
                .iter()
                .find(|(region_position, _)| *region_position == region_of(*position))
                .expect("preflighted consumed-tick region");
            !region.chunks[position]
                .scheduled_block_ticks()
                .starts_with(expected)
        }) {
            return (ResidentHopperTransferCommitResult::Stale, Vec::new());
        }

        let mut changed_chunks = HashSet::new();
        let mut consumed_by_chunk = consumed_by_chunk.into_iter().collect::<Vec<_>>();
        consumed_by_chunk.sort_unstable_by_key(|(position, _)| (position.x, position.z));
        for (position, expected) in consumed_by_chunk {
            let (_, region) = locked_regions
                .iter_mut()
                .find(|(region_position, _)| *region_position == region_of(position))
                .expect("preflighted consumed-tick region");
            let chunk = region
                .chunks
                .get_mut(&position)
                .expect("preflighted hopper tick chunk");
            self.resident
                .read_view
                .update_chunk(position, chunk, |chunk| {
                    assert!(chunk.drain_scheduled_block_tick_prefix(&expected));
                    if let Some(decision_id) = decision_id {
                        chunk.set_world_journal_lsn(decision_id);
                    }
                });
            changed_chunks.insert(position);
        }
        for change in &plan.hoppers {
            let position = chunk_pos_of(change.position);
            let (_, region) = locked_regions
                .iter_mut()
                .find(|(region_position, _)| *region_position == region_of(position))
                .expect("preflighted hopper region");
            let chunk = Arc::make_mut(region.chunks.get_mut(&position).expect("hopper chunk"));
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
            let (_, region) = locked_regions
                .iter_mut()
                .find(|(region_position, _)| *region_position == region_of(position))
                .expect("preflighted chest region");
            let chunk = Arc::make_mut(region.chunks.get_mut(&position).expect("chest chunk"));
            chunk.chests.insert(change.position, change.updated.clone());
            chunk.mark_dirty();
            if let Some(decision_id) = decision_id {
                chunk.set_world_journal_lsn(decision_id);
            }
            changed_chunks.insert(position);
        }
        for change in &plan.furnaces {
            let position = chunk_pos_of(change.position);
            let (_, region) = locked_regions
                .iter_mut()
                .find(|(region_position, _)| *region_position == region_of(position))
                .expect("preflighted furnace region");
            let chunk = Arc::make_mut(region.chunks.get_mut(&position).expect("furnace chunk"));
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
            let (_, region) = locked_regions
                .iter_mut()
                .find(|(region_position, _)| *region_position == region_of(position))
                .expect("preflighted scheduled-tick region");
            let chunk = Arc::make_mut(
                region
                    .chunks
                    .get_mut(&position)
                    .expect("scheduled-tick chunk"),
            );
            if chunk.schedule_block_tick(tick.clone()) {
                if let Some(decision_id) = decision_id {
                    chunk.set_world_journal_lsn(decision_id);
                }
                changed_chunks.insert(position);
            }
        }
        let mut changed_chunks = changed_chunks.into_iter().collect::<Vec<_>>();
        changed_chunks.sort_unstable_by_key(|position| (position.x, position.z));
        if let Some(decision_id) = decision_id {
            for &position in &changed_chunks {
                let (_, region) = locked_regions
                    .iter_mut()
                    .find(|(region_position, _)| *region_position == region_of(position))
                    .expect("changed hopper region remains locked");
                region.pending_journal_lsn.insert(position, decision_id);
            }
        }
        for &position in &changed_chunks {
            let (_, region) = locked_regions
                .iter()
                .find(|(region_position, _)| *region_position == region_of(position))
                .expect("changed hopper region remains locked");
            let chunk = &region.chunks[&position];
            self.resident.publish(position, chunk);
            self.resident.read_view.publish_furnaces(position, chunk);
        }
        (ResidentHopperTransferCommitResult::Applied, changed_chunks)
    }

    pub fn commit_furnace_conditionally(
        &self,
        position: BlockPos,
        expected: &FurnaceBlockEntity,
        updated: &FurnaceBlockEntity,
    ) -> ResidentFurnaceCommitResult {
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
    }

    pub fn commit_chests_conditionally(
        &self,
        positions: &[BlockPos],
        expected: &[ChestBlockEntity],
        updated: &[ChestBlockEntity],
    ) -> ResidentChestCommitResult {
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
                            ticks
                                .into_iter()
                                .filter(|tick| chunk.schedule_fluid_tick(tick.clone()))
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

    fn apply_block_edits_conditionally_inner(
        &self,
        decision_id: Option<u64>,
        plan: ResidentBlockEditPlan<'_>,
    ) -> (ResidentBlockEditBatchResult, Vec<ChunkPos>) {
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

        for tick in scheduled_fluid_ticks {
            let chunk_position = chunk_pos_of(tick.pos);
            let chunk = region
                .chunks
                .get_mut(&chunk_position)
                .expect("preflighted scheduled fluid-tick chunk");
            let added = self
                .resident
                .read_view
                .update_chunk(chunk_position, chunk, |chunk| {
                    let added = chunk.schedule_fluid_tick(tick.clone());
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
}
