use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    AnimalBreedingState, EntityDamage, EntityId, EntityItemStack, EntityKinematics,
    EntityMotionState, EntitySnapshot, EntityStore, EntityView, GoalState, GoalTickStats,
    PathingBudget, PathingProbe, PreparedGoalTick, ResolvedGoalTick, SpawnEntity, Vec3,
    deterministic_uuid, snapshot_from_spawn,
};
#[cfg(any(test, feature = "shadow-compare"))]
use crate::{ShadowComparison, ShadowComparisonStats, ShadowDivergence, ShadowStage};

mod owner_lane;
#[cfg(test)]
mod owner_lane_tests;

use owner_lane::RegionalOwnerLaneReader;
pub use owner_lane::{
    RegionOwnerBatch, RegionOwnerCompletion, RegionOwnerLaneError, RegionOwnerLaneStartError,
    RegionOwnerMutation, RegionalOwnerLane, SequencedRegionMutation,
};

pub const REGION_SIZE_CHUNKS: i32 = 8;
const CHUNK_SIZE_BLOCKS: f64 = 16.0;
const PARALLEL_KINEMATICS_MIN_STATES: usize = 257;
const DIRECT_SELECTED_READ_LIMIT: usize = 16;
static NEXT_REGIONAL_AUTHORITY_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RegionalAuthorityId(u64);

fn next_regional_authority_id() -> RegionalAuthorityId {
    let id = NEXT_REGIONAL_AUTHORITY_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .expect("regional authority id space exhausted");
    RegionalAuthorityId(id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionKey {
    pub x: i32,
    pub z: i32,
}

impl RegionKey {
    #[must_use]
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    #[must_use]
    pub const fn from_chunk(chunk_x: i32, chunk_z: i32) -> Self {
        Self {
            x: chunk_x.div_euclid(REGION_SIZE_CHUNKS),
            z: chunk_z.div_euclid(REGION_SIZE_CHUNKS),
        }
    }

    #[must_use]
    pub fn from_position(position: Vec3) -> Option<Self> {
        if !position.is_finite() {
            return None;
        }
        let chunk_x = (position.x / CHUNK_SIZE_BLOCKS).floor();
        let chunk_z = (position.z / CHUNK_SIZE_BLOCKS).floor();
        if chunk_x < f64::from(i32::MIN)
            || chunk_x > f64::from(i32::MAX)
            || chunk_z < f64::from(i32::MIN)
            || chunk_z > f64::from(i32::MAX)
        {
            return None;
        }
        Some(Self::from_chunk(chunk_x as i32, chunk_z as i32))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionEpoch(pub u64);

impl RegionEpoch {
    pub const INITIAL: Self = Self(1);

    #[must_use]
    pub fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionLease {
    pub key: RegionKey,
    pub epoch: RegionEpoch,
    pub lane: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RegionPhase(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionalDecisionJournalError {
    outcome_unknown: bool,
}

impl RegionalDecisionJournalError {
    pub const SAFE: Self = Self {
        outcome_unknown: false,
    };
    pub const OUTCOME_UNKNOWN: Self = Self {
        outcome_unknown: true,
    };

    #[must_use]
    pub const fn outcome_unknown(self) -> bool {
        self.outcome_unknown
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegionalCommitDecision {
    phase: RegionPhase,
    sequence_watermark: u64,
    upserts: Vec<EntitySnapshot>,
    removed: Vec<EntityId>,
}

impl RegionalCommitDecision {
    pub fn from_parts(
        phase: RegionPhase,
        sequence_watermark: u64,
        mut upserts: Vec<EntitySnapshot>,
        mut removed: Vec<EntityId>,
    ) -> Result<Self, RegionalDecisionJournalError> {
        upserts.sort_unstable_by_key(|snapshot| snapshot.id);
        removed.sort_unstable();
        if upserts.windows(2).any(|pair| pair[0].id == pair[1].id)
            || removed.windows(2).any(|pair| pair[0] == pair[1])
            || upserts
                .iter()
                .any(|snapshot| removed.binary_search(&snapshot.id).is_ok())
        {
            return Err(RegionalDecisionJournalError::SAFE);
        }
        Ok(Self {
            phase,
            sequence_watermark,
            upserts,
            removed,
        })
    }
    #[must_use]
    pub const fn phase(&self) -> RegionPhase {
        self.phase
    }

    #[must_use]
    pub const fn sequence_watermark(&self) -> u64 {
        self.sequence_watermark
    }

    #[must_use]
    pub fn upserts(&self) -> &[EntitySnapshot] {
        &self.upserts
    }

    #[must_use]
    pub fn removed(&self) -> &[EntityId] {
        &self.removed
    }
}

pub trait RegionalDecisionJournal: Send {
    fn enabled(&self) -> bool {
        true
    }

    fn record_commit(
        &mut self,
        decision: &RegionalCommitDecision,
    ) -> Result<(), RegionalDecisionJournalError>;

    fn record_commits(
        &mut self,
        decisions: &[RegionalCommitDecision],
    ) -> Result<(), RegionalDecisionJournalError> {
        for decision in decisions {
            self.record_commit(decision)?;
        }
        Ok(())
    }

    fn clear_commit(&mut self, phase: RegionPhase) -> Result<(), RegionalDecisionJournalError>;

    fn clear_commits(
        &mut self,
        phases: &[RegionPhase],
    ) -> Result<(), RegionalDecisionJournalError> {
        for phase in phases {
            self.clear_commit(*phase)?;
        }
        Ok(())
    }

    fn pending_phases(&self) -> Vec<RegionPhase> {
        Vec::new()
    }

    fn recovery_watermark(&self) -> (RegionPhase, u64) {
        let phase = self
            .pending_phases()
            .into_iter()
            .max()
            .unwrap_or(RegionPhase(0));
        (phase, 0)
    }
}

struct NoopRegionalDecisionJournal;

impl RegionalDecisionJournal for NoopRegionalDecisionJournal {
    fn enabled(&self) -> bool {
        false
    }

    fn record_commit(
        &mut self,
        _decision: &RegionalCommitDecision,
    ) -> Result<(), RegionalDecisionJournalError> {
        Ok(())
    }

    fn clear_commit(&mut self, _phase: RegionPhase) -> Result<(), RegionalDecisionJournalError> {
        Ok(())
    }
}

#[derive(Debug)]
pub struct RegionalOwnerCutoverError {
    pub error: RegionOwnerLaneError,
    pub store: Box<RegionalEntityStore>,
}

#[derive(Debug)]
pub struct RegionalOwnerShutdownError {
    pub error: RegionOwnerLaneError,
    pub recovered: Box<RegionalEntityStore>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegionalOwnerSaveSnapshot {
    sequence_watermark: u64,
    snapshots: Vec<EntitySnapshot>,
    journal_phases: Vec<RegionPhase>,
}

impl RegionalOwnerSaveSnapshot {
    #[must_use]
    pub const fn sequence_watermark(&self) -> u64 {
        self.sequence_watermark
    }

    #[must_use]
    pub fn snapshots(&self) -> &[EntitySnapshot] {
        &self.snapshots
    }

    #[must_use]
    pub fn journal_phases(&self) -> &[RegionPhase] {
        &self.journal_phases
    }

    #[must_use]
    pub fn into_snapshots(self) -> Vec<EntitySnapshot> {
        self.snapshots
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionOwnershipError {
    AlreadyOwned,
    UnknownRegion,
    StaleEpoch,
    EpochExhausted,
    PhaseExhausted,
    PhaseActive,
    PhaseInactive,
    StalePhase,
    UnknownLane,
    DuplicateLaneCompletion,
    LaneCompleted,
    PhaseIncomplete,
}

#[derive(Debug, Default)]
pub struct RegionOwnership {
    owners: BTreeMap<RegionKey, RegionLease>,
    active_phase: Option<RegionPhase>,
    pending_lanes: BTreeSet<usize>,
    last_phase: u64,
}

impl RegionOwnership {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn assign(
        &mut self,
        key: RegionKey,
        lane: usize,
    ) -> Result<RegionLease, RegionOwnershipError> {
        if self.active_phase.is_some() {
            return Err(RegionOwnershipError::PhaseActive);
        }
        if self.owners.contains_key(&key) {
            return Err(RegionOwnershipError::AlreadyOwned);
        }
        let lease = RegionLease {
            key,
            epoch: RegionEpoch::INITIAL,
            lane,
        };
        self.owners.insert(key, lease);
        Ok(lease)
    }

    pub fn reassign(
        &mut self,
        expected: RegionLease,
        lane: usize,
    ) -> Result<RegionLease, RegionOwnershipError> {
        if self.active_phase.is_some() {
            return Err(RegionOwnershipError::PhaseActive);
        }
        let current = self
            .owners
            .get(&expected.key)
            .copied()
            .ok_or(RegionOwnershipError::UnknownRegion)?;
        if current != expected {
            return Err(RegionOwnershipError::StaleEpoch);
        }
        let epoch = current
            .epoch
            .next()
            .ok_or(RegionOwnershipError::EpochExhausted)?;
        let lease = RegionLease {
            key: current.key,
            epoch,
            lane,
        };
        self.owners.insert(current.key, lease);
        Ok(lease)
    }

    fn unassign(&mut self, expected: RegionLease) -> Result<(), RegionOwnershipError> {
        if self.active_phase.is_some() {
            return Err(RegionOwnershipError::PhaseActive);
        }
        if self.owners.get(&expected.key).copied() != Some(expected) {
            return Err(RegionOwnershipError::StaleEpoch);
        }
        self.owners.remove(&expected.key);
        Ok(())
    }

    #[must_use]
    pub fn validate(&self, lease: RegionLease) -> bool {
        self.owners
            .get(&lease.key)
            .is_some_and(|current| *current == lease)
    }

    #[must_use]
    pub fn lease(&self, key: RegionKey) -> Option<RegionLease> {
        self.owners.get(&key).copied()
    }

    pub fn leases(&self) -> impl ExactSizeIterator<Item = RegionLease> + '_ {
        self.owners.values().copied()
    }

    pub fn begin_phase(&mut self) -> Result<RegionPhase, RegionOwnershipError> {
        let lanes = self.owners.values().map(|lease| lease.lane).collect();
        self.begin_phase_for_lanes(lanes)
    }

    fn begin_phase_for_lanes(
        &mut self,
        lanes: BTreeSet<usize>,
    ) -> Result<RegionPhase, RegionOwnershipError> {
        let next = self
            .last_phase
            .checked_add(1)
            .ok_or(RegionOwnershipError::PhaseExhausted)?;
        self.begin_allocated_phase_for_lanes(RegionPhase(next), lanes)
    }

    fn begin_allocated_phase_for_lanes(
        &mut self,
        phase: RegionPhase,
        lanes: BTreeSet<usize>,
    ) -> Result<RegionPhase, RegionOwnershipError> {
        if self.active_phase.is_some() {
            return Err(RegionOwnershipError::PhaseActive);
        }
        if lanes
            .iter()
            .any(|lane| !self.owners.values().any(|lease| lease.lane == *lane))
        {
            return Err(RegionOwnershipError::UnknownLane);
        }
        if phase.0 <= self.last_phase {
            return Err(RegionOwnershipError::PhaseExhausted);
        }
        self.last_phase = phase.0;
        self.active_phase = Some(phase);
        self.pending_lanes = lanes;
        Ok(phase)
    }

    pub fn acknowledge_lane(
        &mut self,
        expected: RegionPhase,
        lane: usize,
    ) -> Result<(), RegionOwnershipError> {
        let Some(active) = self.active_phase else {
            return Err(RegionOwnershipError::PhaseInactive);
        };
        if active != expected {
            return Err(RegionOwnershipError::StalePhase);
        }
        if self.pending_lanes.remove(&lane) {
            return Ok(());
        }
        if self.owners.values().any(|lease| lease.lane == lane) {
            Err(RegionOwnershipError::DuplicateLaneCompletion)
        } else {
            Err(RegionOwnershipError::UnknownLane)
        }
    }

    pub fn finish_phase(&mut self, expected: RegionPhase) -> Result<(), RegionOwnershipError> {
        self.validate_finish(expected)?;
        self.active_phase = None;
        Ok(())
    }

    fn validate_finish(&self, expected: RegionPhase) -> Result<(), RegionOwnershipError> {
        let Some(active) = self.active_phase else {
            return Err(RegionOwnershipError::PhaseInactive);
        };
        if active != expected {
            return Err(RegionOwnershipError::StalePhase);
        }
        if !self.pending_lanes.is_empty() {
            return Err(RegionOwnershipError::PhaseIncomplete);
        }
        Ok(())
    }

    fn validate_lane(
        &self,
        expected: RegionPhase,
        lane: usize,
    ) -> Result<(), RegionOwnershipError> {
        let Some(active) = self.active_phase else {
            return Err(RegionOwnershipError::PhaseInactive);
        };
        if active != expected {
            return Err(RegionOwnershipError::StalePhase);
        }
        if self.pending_lanes.contains(&lane) {
            return Ok(());
        }
        if self.owners.values().any(|lease| lease.lane == lane) {
            Err(RegionOwnershipError::LaneCompleted)
        } else {
            Err(RegionOwnershipError::UnknownLane)
        }
    }

    #[must_use]
    pub fn validate_phase(&self, phase: RegionPhase) -> bool {
        self.active_phase == Some(phase)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionEntityStoreError {
    Ownership(RegionOwnershipError),
    StalePhase,
    StaleLease,
    WrongSpawnRegion,
    WrongSourceRegion,
    UnknownEntity,
    DuplicateUuid,
    IdExhausted,
    CrossRegionReference,
    TargetConflict,
    SameRegionTransfer,
    WrongTargetRegion,
    InvalidKinematics,
    TransferConflict,
    UnknownTransfer,
    DecisionConflict,
    TransferUndecided,
    SourceChanged,
}

impl From<RegionOwnershipError> for RegionEntityStoreError {
    fn from(error: RegionOwnershipError) -> Self {
        Self::Ownership(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransferId {
    pub tick: u64,
    pub source: RegionKey,
    pub source_epoch: RegionEpoch,
    pub entity: EntityId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDecision {
    Commit,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferApply {
    Committed,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionalKinematicsApply {
    AppliedLocal,
    PreparedTransfer(TransferId),
}

#[derive(Debug, Clone, PartialEq)]
struct PreparedTransfer {
    phase: RegionPhase,
    source: RegionLease,
    target: RegionLease,
    source_snapshots: Vec<EntitySnapshot>,
    target_snapshots: Vec<EntitySnapshot>,
    decision: Option<TransferDecision>,
    applied: Option<TransferApply>,
}

#[derive(Debug)]
struct PendingInsert {
    key: RegionKey,
    snapshot: EntitySnapshot,
}

#[derive(Debug)]
pub struct RegionalPreparedGoalTick {
    authority: RegionalAuthorityId,
    phase: RegionPhase,
    leases: BTreeMap<RegionKey, RegionLease>,
    batches: BTreeMap<RegionKey, PreparedGoalTick>,
    follow_targets: BTreeMap<RegionKey, HashMap<EntityId, Vec3>>,
    follow_target_sources: BTreeMap<EntityId, RegionalFollowTargetSource>,
    expected_missing_follow_targets: HashSet<EntityId>,
    goal_inputs: BTreeMap<EntityId, EntitySnapshot>,
}

#[derive(Debug)]
pub struct RegionalResolvedGoalTick {
    authority: RegionalAuthorityId,
    phase: RegionPhase,
    leases: BTreeMap<RegionKey, RegionLease>,
    batches: BTreeMap<RegionKey, ResolvedGoalTick>,
    follow_targets: BTreeMap<RegionKey, HashMap<EntityId, Vec3>>,
    follow_target_sources: BTreeMap<EntityId, RegionalFollowTargetSource>,
    expected_missing_follow_targets: HashSet<EntityId>,
    goal_inputs: BTreeMap<EntityId, EntitySnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
struct RegionalFollowTargetSource {
    region: RegionKey,
    snapshot: EntitySnapshot,
}

struct CapturedFollowTargets {
    remote_by_region: BTreeMap<RegionKey, HashMap<EntityId, Vec3>>,
    sources: BTreeMap<EntityId, RegionalFollowTargetSource>,
    expected_missing_follow_targets: HashSet<EntityId>,
    inputs: BTreeMap<EntityId, EntitySnapshot>,
}

#[cfg(any(test, feature = "shadow-compare"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionalShadowBatchError {
    TickMismatch,
    StageMismatch,
    DuplicateRegion,
}

#[cfg(any(test, feature = "shadow-compare"))]
#[derive(Debug)]
enum RegionalShadowRegionOutcome {
    Match(ShadowComparison),
    Divergence(Box<ShadowDivergence>),
}

#[cfg(any(test, feature = "shadow-compare"))]
#[derive(Debug)]
pub struct RegionalShadowComparisonBatch {
    tick: u64,
    stage: ShadowStage,
    regions: BTreeMap<RegionKey, RegionalShadowRegionOutcome>,
}

#[cfg(any(test, feature = "shadow-compare"))]
#[derive(Debug)]
pub struct RegionalShadowComparisonOutcome {
    comparison: ShadowComparison,
    first_divergence: Option<Box<ShadowDivergence>>,
}

#[cfg(any(test, feature = "shadow-compare"))]
impl RegionalShadowComparisonBatch {
    #[must_use]
    pub fn new(tick: u64, stage: ShadowStage) -> Self {
        Self {
            tick,
            stage,
            regions: BTreeMap::new(),
        }
    }

    pub fn compare_region(
        &mut self,
        key: RegionKey,
        store: &mut EntityStore,
    ) -> Result<(), RegionalShadowBatchError> {
        if self.regions.contains_key(&key) {
            return Err(RegionalShadowBatchError::DuplicateRegion);
        }
        let outcome = match store.compare_shadow(self.tick, self.stage) {
            Ok(comparison) => RegionalShadowRegionOutcome::Match(comparison),
            Err(divergence) => RegionalShadowRegionOutcome::Divergence(divergence),
        };
        self.regions.insert(key, outcome);
        Ok(())
    }

    pub fn merge(&mut self, other: Self) -> Result<(), RegionalShadowBatchError> {
        if self.tick != other.tick {
            return Err(RegionalShadowBatchError::TickMismatch);
        }
        if self.stage != other.stage {
            return Err(RegionalShadowBatchError::StageMismatch);
        }
        if other
            .regions
            .keys()
            .any(|key| self.regions.contains_key(key))
        {
            return Err(RegionalShadowBatchError::DuplicateRegion);
        }
        self.regions.extend(other.regions);
        Ok(())
    }

    #[must_use]
    pub fn finish(self) -> RegionalShadowComparisonOutcome {
        let mut compared_entities = 0usize;
        let mut compared_events = 0usize;
        let mut first_divergence = None;
        for outcome in self.regions.into_values() {
            match outcome {
                RegionalShadowRegionOutcome::Match(comparison) => {
                    compared_entities =
                        compared_entities.saturating_add(comparison.compared_entities);
                    compared_events = compared_events.saturating_add(comparison.compared_events);
                }
                RegionalShadowRegionOutcome::Divergence(divergence) => {
                    compared_entities =
                        compared_entities.saturating_add(divergence.compared_entities);
                    compared_events = compared_events.saturating_add(divergence.compared_events);
                    if first_divergence.is_none() {
                        first_divergence = Some(divergence);
                    }
                }
            }
        }
        RegionalShadowComparisonOutcome {
            comparison: ShadowComparison {
                tick: self.tick,
                stage: self.stage,
                compared_entities,
                compared_events,
            },
            first_divergence,
        }
    }
}

impl RegionalPreparedGoalTick {
    #[must_use]
    pub fn parallel_batch_count(&self) -> usize {
        self.batches
            .values()
            .filter(|batch| batch.pathing_request_count() > 0)
            .count()
    }

    pub fn visit_pathing_probe_positions(
        &self,
        budget: PathingBudget,
        mut visitor: impl FnMut(EntityId, Vec3),
    ) {
        for batch in self.batches.values() {
            batch.visit_pathing_probe_positions(budget, &mut visitor);
        }
    }

    #[must_use]
    pub fn resolve(
        self,
        probe: &dyn PathingProbe,
        budget: PathingBudget,
    ) -> RegionalResolvedGoalTick {
        RegionalResolvedGoalTick {
            authority: self.authority,
            phase: self.phase,
            leases: self.leases,
            follow_targets: self.follow_targets,
            follow_target_sources: self.follow_target_sources,
            expected_missing_follow_targets: self.expected_missing_follow_targets,
            goal_inputs: self.goal_inputs,
            batches: self
                .batches
                .into_iter()
                .map(|(key, prepared)| (key, prepared.resolve(probe, budget)))
                .collect(),
        }
    }

    #[must_use]
    pub fn resolve_parallel(
        self,
        probe: &(dyn PathingProbe + Sync),
        budget: PathingBudget,
        max_workers: usize,
    ) -> RegionalResolvedGoalTick {
        let worker_count = max_workers.max(1).min(self.parallel_batch_count().max(1));
        if worker_count == 1 {
            return self.resolve(probe, budget);
        }

        let Self {
            authority,
            phase,
            leases,
            batches,
            follow_targets,
            follow_target_sources,
            expected_missing_follow_targets,
            goal_inputs,
        } = self;
        let batch_count = batches.len();
        let (parallel_batches, inline_batches): (Vec<_>, Vec<_>) = batches
            .into_iter()
            .partition(|(_, batch)| batch.pathing_request_count() > 0);
        let mut buckets = (0..worker_count).map(|_| Vec::new()).collect::<Vec<_>>();
        for (index, batch) in parallel_batches.into_iter().enumerate() {
            buckets[index % worker_count].push(batch);
        }
        let local = buckets.pop().expect("positive regional worker count");
        let resolved = std::sync::Mutex::new(Vec::with_capacity(batch_count));
        rayon::scope(|scope| {
            for bucket in buckets {
                let resolved = &resolved;
                scope.spawn(move |_| {
                    let worker_results = bucket
                        .into_iter()
                        .map(|(key, prepared)| (key, prepared.resolve(probe, budget)))
                        .collect::<Vec<_>>();
                    resolved
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .extend(worker_results);
                });
            }
            let local_results = inline_batches
                .into_iter()
                .chain(local)
                .map(|(key, prepared)| (key, prepared.resolve(probe, budget)))
                .collect::<Vec<_>>();
            resolved
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .extend(local_results);
        });
        let resolved = resolved
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .into_iter()
            .collect::<BTreeMap<_, _>>();

        RegionalResolvedGoalTick {
            authority,
            phase,
            leases,
            batches: resolved,
            follow_targets,
            follow_target_sources,
            expected_missing_follow_targets,
            goal_inputs,
        }
    }
}

#[derive(Debug)]
pub struct RegionalEntityStore {
    authority: RegionalAuthorityId,
    ownership: RegionOwnership,
    stores: BTreeMap<RegionKey, EntityStore>,
    locations: BTreeMap<EntityId, RegionKey>,
    uuids: HashMap<Uuid, EntityId>,
    transfers: BTreeMap<TransferId, PreparedTransfer>,
    in_flight_transfers: BTreeMap<EntityId, TransferId>,
    #[cfg(any(test, feature = "shadow-compare"))]
    shadow_stats: ShadowComparisonStats,
    next_id: i32,
}

pub struct RegionalOwnerCoordinator {
    authority: RegionalAuthorityId,
    ownership: RegionOwnership,
    locations: BTreeMap<EntityId, RegionKey>,
    uuids: HashMap<Uuid, EntityId>,
    vehicle_passengers: HashMap<EntityId, EntityId>,
    passenger_vehicles: HashMap<EntityId, EntityId>,
    transfers: BTreeMap<TransferId, PreparedTransfer>,
    in_flight_transfers: BTreeMap<EntityId, TransferId>,
    #[cfg(any(test, feature = "shadow-compare"))]
    shadow_stats: ShadowComparisonStats,
    next_id: i32,
    lanes: BTreeMap<usize, RegionalOwnerLane>,
    commit_state: Arc<RegionalOwnerCommitState>,
}

struct RegionalOwnerCommitState {
    next_phase: AtomicU64,
    next_sequence: AtomicU64,
    journal: Mutex<Box<dyn RegionalDecisionJournal>>,
    journal_phases: Mutex<BTreeMap<RegionPhase, u64>>,
    outcome_unknown: AtomicBool,
    #[cfg(test)]
    save_barrier_phase_snapshot_hook: Mutex<Option<SaveBarrierPhaseSnapshotHook>>,
}

#[cfg(test)]
struct SaveBarrierPhaseSnapshotHook {
    entered: SyncSender<()>,
    release: Receiver<()>,
}

impl RegionalOwnerCommitState {
    fn reserve_sequences(&self, count: usize) -> Result<(u64, u64), RegionOwnerLaneError> {
        if self.outcome_unknown.load(Ordering::Acquire) {
            return Err(RegionOwnerLaneError::Busy);
        }
        let count = u64::try_from(count).map_err(|_| RegionOwnerLaneError::InvalidMutation)?;
        let start = self
            .next_sequence
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(count)
            })
            .map_err(|_| RegionOwnerLaneError::InvalidMutation)?;
        Ok((start, start + count))
    }

    fn sequence_watermark(&self) -> u64 {
        self.next_sequence.load(Ordering::Acquire)
    }

    fn reserve_phase(&self) -> Result<RegionPhase, RegionOwnerLaneError> {
        if self.outcome_unknown.load(Ordering::Acquire) {
            return Err(RegionOwnerLaneError::Busy);
        }
        self.next_phase
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(1)
            })
            .map(|phase| RegionPhase(phase + 1))
            .map_err(|_| RegionOwnerLaneError::InvalidMutation)
    }

    fn clear_recovered_commits(&self, phases: &[RegionPhase]) -> Result<(), RegionOwnerLaneError> {
        self.journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear_commits(phases)
            .map_err(|_| RegionOwnerLaneError::Journal)?;
        let mut journal_phases = self
            .journal_phases
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for phase in phases {
            journal_phases.remove(phase);
        }
        Ok(())
    }

    fn pending_journal_phases(&self, sequence_watermark: u64) -> Vec<RegionPhase> {
        #[cfg(test)]
        if let Some(hook) = self
            .save_barrier_phase_snapshot_hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            hook.entered
                .send(())
                .expect("save barrier phase snapshot observer dropped");
            hook.release
                .recv()
                .expect("save barrier phase snapshot release dropped");
        }
        self.journal_phases
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .filter_map(|(&phase, &sequence)| (sequence <= sequence_watermark).then_some(phase))
            .collect()
    }

    fn record_commit(
        &self,
        decision: &RegionalCommitDecision,
    ) -> Result<(), RegionalDecisionJournalError> {
        self.journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .record_commit(decision)?;
        self.journal_phases
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(decision.phase(), decision.sequence_watermark());
        Ok(())
    }

    #[cfg(test)]
    fn pause_before_save_barrier_phase_snapshot(
        &self,
        entered: SyncSender<()>,
        release: Receiver<()>,
    ) {
        *self
            .save_barrier_phase_snapshot_hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(SaveBarrierPhaseSnapshotHook { entered, release });
    }
}

#[derive(Clone)]
pub struct RegionalOwnerHandle {
    sender: SyncSender<RegionalOwnerCommand>,
    authority: RegionalAuthorityId,
    selected_read_routes: Arc<RwLock<HashMap<EntityId, CachedEntityReadRoute>>>,
    entity_state_version: Arc<AtomicU64>,
    active_entity_writers: Arc<AtomicUsize>,
    mutation_gate: Arc<RwLock<()>>,
    direct_selected_reads: Arc<AtomicUsize>,
    commit_state: Arc<RegionalOwnerCommitState>,
    #[cfg(test)]
    selected_read_probe: Arc<std::sync::Mutex<Option<SelectedReadProbe>>>,
    #[cfg(test)]
    referenced_goal_fallback_probe: Arc<std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>>,
}

#[cfg(test)]
struct SelectedReadProbe {
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

struct DirectSelectedReadPermit {
    active: Arc<AtomicUsize>,
}

impl DirectSelectedReadPermit {
    fn try_acquire(active: Arc<AtomicUsize>) -> Option<Self> {
        active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < DIRECT_SELECTED_READ_LIMIT).then_some(current + 1)
            })
            .ok()?;
        Some(Self { active })
    }
}

impl Drop for DirectSelectedReadPermit {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::Release);
    }
}

struct EntityWriterPermit {
    active: Arc<AtomicUsize>,
    version: Arc<AtomicU64>,
}

impl EntityWriterPermit {
    fn enter(active: Arc<AtomicUsize>, version: Arc<AtomicU64>) -> Self {
        active.fetch_add(1, Ordering::AcqRel);
        Self { active, version }
    }
}

impl Drop for EntityWriterPermit {
    fn drop(&mut self) {
        self.version.fetch_add(1, Ordering::Release);
        self.active.fetch_sub(1, Ordering::Release);
    }
}

#[derive(Clone)]
struct CachedEntityReadRoute {
    lease: RegionLease,
    uuid: Uuid,
    standalone: bool,
    owner: RegionalOwnerLaneReader,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegionalOwnerStatus {
    pub entity_count: usize,
    pub lane_count: usize,
    #[cfg(any(test, feature = "shadow-compare"))]
    pub shadow: ShadowComparisonStats,
}

#[derive(Debug, Clone)]
pub struct VersionedEntitySnapshots {
    authority: RegionalAuthorityId,
    version: u64,
    snapshots: Vec<EntitySnapshot>,
}

impl VersionedEntitySnapshots {
    #[must_use]
    pub fn snapshots(&self) -> &[EntitySnapshot] {
        &self.snapshots
    }

    fn into_snapshots(self) -> Vec<EntitySnapshot> {
        self.snapshots
    }
}

pub struct RegionalOwnerRuntime {
    handle: RegionalOwnerHandle,
    worker: Option<JoinHandle<()>>,
}

#[derive(Debug)]
pub enum RegionalOwnerRuntimeShutdownError {
    Closed,
    WorkerPanicked,
    Owner(RegionalOwnerShutdownError),
}

type GoalApplyKinematicsResult =
    Result<Option<(GoalTickStats, Vec<EntityKinematics>)>, RegionOwnerLaneError>;

enum RegionalOwnerCommand {
    Snapshot {
        entity: EntityId,
        reply: std::sync::mpsc::Sender<Result<Option<EntitySnapshot>, RegionOwnerLaneError>>,
    },
    Snapshots {
        reply: std::sync::mpsc::Sender<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>>,
    },
    SnapshotsForIds {
        entities: HashSet<EntityId>,
        reply: std::sync::mpsc::Sender<Result<VersionedEntitySnapshots, RegionOwnerLaneError>>,
    },
    ContainsUuid {
        uuid: Uuid,
        reply: std::sync::mpsc::Sender<bool>,
    },
    BreedingTickSnapshots {
        reply: std::sync::mpsc::Sender<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>>,
    },
    Status {
        reply: std::sync::mpsc::Sender<RegionalOwnerStatus>,
    },
    ReconfigureLanes {
        lane_count: usize,
        reply: std::sync::mpsc::Sender<Result<usize, RegionOwnerLaneError>>,
    },
    SaveBarrier {
        reply: std::sync::mpsc::Sender<Result<RegionalOwnerSaveSnapshot, RegionOwnerLaneError>>,
    },
    SpawnAuthoritative {
        entity: Box<SpawnEntity>,
        defer_journal: bool,
        reply: std::sync::mpsc::Sender<Result<EntityId, RegionOwnerLaneError>>,
    },
    SpawnAuthoritativeBatch {
        entities: Vec<SpawnEntity>,
        reply: std::sync::mpsc::Sender<Result<Vec<EntityId>, RegionOwnerLaneError>>,
    },
    SpawnUniqueAuthoritativeBatch {
        entities: Vec<SpawnEntity>,
        reply: std::sync::mpsc::Sender<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>>,
    },
    InsertAuthoritativeSnapshots {
        snapshots: Vec<EntitySnapshot>,
        reply: std::sync::mpsc::Sender<Result<usize, RegionOwnerLaneError>>,
    },
    Remove {
        entity: EntityId,
        reply: std::sync::mpsc::Sender<Result<Option<EntitySnapshot>, RegionOwnerLaneError>>,
    },
    RemoveIfCurrent {
        expected: Box<EntitySnapshot>,
        reply: std::sync::mpsc::Sender<Result<Option<EntitySnapshot>, RegionOwnerLaneError>>,
    },
    SetAnimalStatesIfCurrent {
        states: Vec<(EntitySnapshot, AnimalBreedingState)>,
        defer_journal: bool,
        reply: std::sync::mpsc::Sender<Result<bool, RegionOwnerLaneError>>,
    },
    SetGoal {
        entity: EntityId,
        goal: GoalState,
        reply: std::sync::mpsc::Sender<Result<bool, RegionOwnerLaneError>>,
    },
    SetGoals {
        goals: Vec<(EntityId, GoalState)>,
        defer_journal: bool,
        reply: std::sync::mpsc::Sender<Result<usize, RegionOwnerLaneError>>,
    },
    SetItemStack {
        entity: EntityId,
        item_stack: Option<EntityItemStack>,
        reply: std::sync::mpsc::Sender<Result<bool, RegionOwnerLaneError>>,
    },
    SetItemStackIfCurrent {
        expected: Box<EntitySnapshot>,
        item_stack: Option<EntityItemStack>,
        reply: std::sync::mpsc::Sender<Result<bool, RegionOwnerLaneError>>,
    },
    SetPosition {
        entity: EntityId,
        position: Vec3,
        reply: std::sync::mpsc::Sender<Result<bool, RegionOwnerLaneError>>,
    },
    SetVelocities {
        velocities: Vec<(EntityId, Vec3)>,
        reply: std::sync::mpsc::Sender<Result<(), RegionOwnerLaneError>>,
    },
    ApplyKinematicsIfCurrent {
        states: Vec<(EntitySnapshot, EntityKinematics)>,
        defer_journal: bool,
        reply: std::sync::mpsc::Sender<Result<bool, RegionOwnerLaneError>>,
    },
    DamageIfCurrent {
        expected: Box<EntitySnapshot>,
        amount: f32,
        reply: std::sync::mpsc::Sender<Result<Option<EntityDamage>, RegionOwnerLaneError>>,
    },
    Damage {
        entity: EntityId,
        amount: f32,
        reply: std::sync::mpsc::Sender<Result<Option<EntityDamage>, RegionOwnerLaneError>>,
    },
    PrepareGoalTick {
        tick: u64,
        active_ids: HashSet<EntityId>,
        selected: Option<VersionedEntitySnapshots>,
        reply: std::sync::mpsc::Sender<Result<RegionalPreparedGoalTick, RegionOwnerLaneError>>,
    },
    ApplyPreparedGoalTick {
        resolved: Box<RegionalResolvedGoalTick>,
        reply: std::sync::mpsc::Sender<Result<GoalTickStats, RegionOwnerLaneError>>,
    },
    ApplyPreparedGoalTickAndKinematics {
        resolved: Box<RegionalResolvedGoalTick>,
        entities: HashSet<EntityId>,
        defer_journal: bool,
        reply: std::sync::mpsc::Sender<GoalApplyKinematicsResult>,
    },
    #[cfg(any(test, feature = "shadow-compare"))]
    CompareShadow {
        tick: u64,
        stage: ShadowStage,
        reply: std::sync::mpsc::Sender<
            Result<Result<ShadowComparison, Box<ShadowDivergence>>, RegionOwnerLaneError>,
        >,
    },
    #[cfg(test)]
    HoldForTest {
        entered: std::sync::mpsc::Sender<()>,
        release: Receiver<()>,
    },
    Shutdown {
        reply: std::sync::mpsc::Sender<Result<RegionalEntityStore, RegionalOwnerShutdownError>>,
    },
}

impl RegionalOwnerCommand {
    fn mutates_entity_snapshots(&self) -> bool {
        matches!(
            self,
            Self::SpawnAuthoritative { .. }
                | Self::SpawnAuthoritativeBatch { .. }
                | Self::SpawnUniqueAuthoritativeBatch { .. }
                | Self::InsertAuthoritativeSnapshots { .. }
                | Self::Remove { .. }
                | Self::RemoveIfCurrent { .. }
                | Self::SetAnimalStatesIfCurrent { .. }
                | Self::SetGoal { .. }
                | Self::SetGoals { .. }
                | Self::SetItemStack { .. }
                | Self::SetItemStackIfCurrent { .. }
                | Self::SetPosition { .. }
                | Self::SetVelocities { .. }
                | Self::ApplyKinematicsIfCurrent { .. }
                | Self::DamageIfCurrent { .. }
                | Self::Damage { .. }
                | Self::ApplyPreparedGoalTick { .. }
                | Self::ApplyPreparedGoalTickAndKinematics { .. }
        )
    }

    fn requires_exclusive_lane_access(&self) -> bool {
        let exclusive = self.mutates_entity_snapshots()
            || matches!(
                self,
                Self::Snapshot { .. }
                    | Self::Snapshots { .. }
                    | Self::SnapshotsForIds { .. }
                    | Self::BreedingTickSnapshots { .. }
                    | Self::PrepareGoalTick { .. }
                    | Self::ReconfigureLanes { .. }
                    | Self::SaveBarrier { .. }
                    | Self::Shutdown { .. }
            );
        #[cfg(any(test, feature = "shadow-compare"))]
        {
            exclusive || matches!(self, Self::CompareShadow { .. })
        }
        #[cfg(not(any(test, feature = "shadow-compare")))]
        {
            exclusive
        }
    }
}

impl RegionalOwnerHandle {
    pub fn snapshot(
        &self,
        entity: EntityId,
    ) -> Result<Option<EntitySnapshot>, RegionOwnerLaneError> {
        if let Some(mut snapshots) = self.read_cached_selected_entities(&HashSet::from([entity])) {
            return Ok(snapshots.pop());
        }
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::Snapshot { entity, reply })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn snapshots(&self) -> Result<Vec<EntitySnapshot>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::Snapshots { reply })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn snapshots_for_ids(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<Vec<EntitySnapshot>, RegionOwnerLaneError> {
        self.snapshots_for_ids_versioned(entities)
            .map(VersionedEntitySnapshots::into_snapshots)
    }

    pub fn snapshots_for_ids_versioned(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<VersionedEntitySnapshots, RegionOwnerLaneError> {
        if let Some((version, snapshots)) = self.read_cached_selected_entities_versioned(entities) {
            return Ok(VersionedEntitySnapshots {
                authority: self.authority,
                version,
                snapshots,
            });
        }
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SnapshotsForIds {
                entities: entities.clone(),
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    fn read_cached_selected_entities(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Option<Vec<EntitySnapshot>> {
        self.read_cached_selected_entities_versioned(entities)
            .map(|(_, snapshots)| snapshots)
    }

    fn read_cached_selected_entities_versioned(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Option<(u64, Vec<EntitySnapshot>)> {
        let version_before = self.entity_state_version.load(Ordering::Acquire);
        if self.active_entity_writers.load(Ordering::Acquire) != 0 {
            return None;
        }
        let _admission =
            DirectSelectedReadPermit::try_acquire(Arc::clone(&self.direct_selected_reads))?;
        let routes = self
            .selected_read_routes
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut ordered = entities.iter().copied().collect::<Vec<_>>();
        ordered.sort_unstable();
        let mut expected = HashMap::with_capacity(ordered.len());
        let mut requests =
            BTreeMap::<usize, (RegionalOwnerLaneReader, Vec<(RegionLease, EntityId)>)>::new();
        for entity in ordered {
            let route = routes.get(&entity)?.clone();
            expected.insert(entity, (route.lease, route.uuid));
            requests
                .entry(route.lease.lane)
                .or_insert_with(|| (route.owner.clone(), Vec::new()))
                .1
                .push((route.lease, entity));
        }
        drop(routes);

        let mut pending = Vec::with_capacity(requests.len());
        for (_, (owner, entities)) in requests {
            pending.push(owner.request_snapshots_for_ids(entities).ok()?);
        }
        #[cfg(test)]
        if let Some(probe) = self
            .selected_read_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            probe.entered.send(()).ok()?;
            probe.release.recv().ok()?;
        }
        let mut snapshots = Vec::with_capacity(expected.len());
        for completion in pending {
            snapshots.extend(completion.recv().ok()?.ok()?);
        }
        if snapshots.len() != expected.len() {
            return None;
        }
        snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
        let mut seen = HashSet::with_capacity(snapshots.len());
        for snapshot in &snapshots {
            let (lease, uuid) = expected.get(&snapshot.id)?;
            if !seen.insert(snapshot.id)
                || snapshot.uuid != *uuid
                || RegionKey::from_position(snapshot.position) != Some(lease.key)
            {
                return None;
            }
        }
        if self.active_entity_writers.load(Ordering::Acquire) != 0 {
            return None;
        }
        let version_after = self.entity_state_version.load(Ordering::Acquire);
        if version_after != version_before {
            #[cfg(test)]
            self.notify_referenced_goal_fallback_probe();
            return None;
        }
        Some((version_after, snapshots))
    }

    #[cfg(test)]
    fn pause_selected_read_after_dispatch_for_test(
        &self,
        entered: std::sync::mpsc::Sender<()>,
        release: Receiver<()>,
    ) {
        *self
            .selected_read_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(SelectedReadProbe { entered, release });
    }

    #[cfg(test)]
    fn notify_referenced_goal_fallback_for_test(&self, entered: std::sync::mpsc::Sender<()>) {
        *self
            .referenced_goal_fallback_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(entered);
    }

    #[cfg(test)]
    fn notify_referenced_goal_fallback_probe(&self) {
        if let Some(probe) = self
            .referenced_goal_fallback_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = probe.send(());
        }
    }

    #[cfg(test)]
    fn hold_coordinator_for_test(
        &self,
        entered: std::sync::mpsc::Sender<()>,
        release: Receiver<()>,
    ) -> Result<(), RegionOwnerLaneError> {
        self.sender
            .send(RegionalOwnerCommand::HoldForTest { entered, release })
            .map_err(|_| RegionOwnerLaneError::Closed)
    }

    pub fn contains_uuid(&self, uuid: Uuid) -> Result<bool, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::ContainsUuid { uuid, reply })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)
    }

    pub fn breeding_tick_snapshots(&self) -> Result<Vec<EntitySnapshot>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::BreedingTickSnapshots { reply })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn status(&self) -> Result<RegionalOwnerStatus, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::Status { reply })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)
    }

    pub fn reconfigure_lanes(&self, lane_count: usize) -> Result<usize, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::ReconfigureLanes { lane_count, reply })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn clear_recovered_commits(
        &self,
        phases: impl IntoIterator<Item = RegionPhase>,
    ) -> Result<(), RegionOwnerLaneError> {
        self.commit_state
            .clear_recovered_commits(&phases.into_iter().collect::<Vec<_>>())
    }

    pub fn save_barrier(&self) -> Result<RegionalOwnerSaveSnapshot, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SaveBarrier { reply })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn spawn_authoritative(
        &self,
        entity: SpawnEntity,
    ) -> Result<EntityId, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SpawnAuthoritative {
                entity: Box::new(entity),
                defer_journal: false,
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn spawn_authoritative_deferred_journal(
        &self,
        entity: SpawnEntity,
    ) -> Result<EntityId, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SpawnAuthoritative {
                entity: Box::new(entity),
                defer_journal: true,
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn spawn_authoritative_batch(
        &self,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Result<Vec<EntityId>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SpawnAuthoritativeBatch {
                entities: entities.into_iter().collect(),
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn spawn_unique_authoritative_batch(
        &self,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Result<Vec<EntitySnapshot>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SpawnUniqueAuthoritativeBatch {
                entities: entities.into_iter().collect(),
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn insert_authoritative_snapshots_batch(
        &self,
        snapshots: impl IntoIterator<Item = EntitySnapshot>,
    ) -> Result<usize, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::InsertAuthoritativeSnapshots {
                snapshots: snapshots.into_iter().collect(),
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn remove(&self, entity: EntityId) -> Result<Option<EntitySnapshot>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::Remove { entity, reply })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn remove_if_current(
        &self,
        expected: EntitySnapshot,
    ) -> Result<Option<EntitySnapshot>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::RemoveIfCurrent {
                expected: Box::new(expected),
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn set_animal_states_if_current(
        &self,
        states: impl IntoIterator<Item = (EntitySnapshot, AnimalBreedingState)>,
    ) -> Result<bool, RegionOwnerLaneError> {
        self.set_animal_states_if_current_inner(states, true)
    }

    pub fn set_animal_states_if_current_deferred_journal(
        &self,
        states: impl IntoIterator<Item = (EntitySnapshot, AnimalBreedingState)>,
    ) -> Result<bool, RegionOwnerLaneError> {
        self.set_animal_states_if_current_inner(states, false)
    }

    fn set_animal_states_if_current_inner(
        &self,
        states: impl IntoIterator<Item = (EntitySnapshot, AnimalBreedingState)>,
        journal_commit: bool,
    ) -> Result<bool, RegionOwnerLaneError> {
        let states = states.into_iter().collect::<Vec<_>>();
        if states.is_empty() {
            return Ok(true);
        }
        let expected_count = states.len();
        let direct = states
            .iter()
            .map(|(expected, animal)| {
                (
                    expected.clone(),
                    RegionOwnerMutation::SetAnimalStateIfCurrent {
                        expected: Box::new(expected.clone()),
                        animal: *animal,
                    },
                )
            })
            .collect();
        if let Some(result) =
            self.try_commit_cached_snapshot_mutations_with_journal(direct, journal_commit)
        {
            return result.map(|snapshots| snapshots.len() == expected_count);
        }
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SetAnimalStatesIfCurrent {
                states,
                defer_journal: !journal_commit,
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn set_goal(
        &self,
        entity: EntityId,
        goal: GoalState,
    ) -> Result<bool, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SetGoal {
                entity,
                goal,
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn set_goals(
        &self,
        goals: impl IntoIterator<Item = (EntityId, GoalState)>,
    ) -> Result<usize, RegionOwnerLaneError> {
        let goals = goals.into_iter().collect::<Vec<_>>();
        if goals.is_empty() {
            return Ok(0);
        }
        let ids = goals
            .iter()
            .map(|(entity, _)| *entity)
            .collect::<HashSet<_>>();
        let mut selected = ids.clone();
        selected.extend(goals.iter().filter_map(|(_, goal)| goal_reference(goal)));
        if ids.len() == goals.len()
            && let Some((version, snapshots)) =
                self.read_cached_selected_entities_versioned(&selected)
        {
            let current = snapshots
                .into_iter()
                .map(|snapshot| (snapshot.id, snapshot))
                .collect::<HashMap<_, _>>();
            let direct = goals
                .iter()
                .map(|(entity, goal)| {
                    let expected = current[entity].clone();
                    (
                        expected.clone(),
                        RegionOwnerMutation::SetGoalIfCurrent {
                            expected: Box::new(expected),
                            goal: goal.clone(),
                        },
                    )
                })
                .collect();
            let referenced = goals.iter().any(|(_, goal)| goal_reference(goal).is_some());
            let result = if referenced {
                self.try_commit_cached_referenced_snapshot_mutations(direct, version)
            } else {
                self.try_commit_cached_snapshot_mutations(direct)
            };
            if let Some(result) = result {
                return result.map(|snapshots| snapshots.len());
            }
        }

        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SetGoals {
                goals,
                defer_journal: false,
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn set_goals_deferred_journal(
        &self,
        goals: impl IntoIterator<Item = (EntityId, GoalState)>,
    ) -> Result<usize, RegionOwnerLaneError> {
        let goals = goals.into_iter().collect::<Vec<_>>();
        if goals.is_empty() {
            return Ok(0);
        }
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SetGoals {
                goals,
                defer_journal: true,
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn set_item_stack(
        &self,
        entity: EntityId,
        item_stack: Option<EntityItemStack>,
    ) -> Result<bool, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SetItemStack {
                entity,
                item_stack,
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn set_item_stack_if_current(
        &self,
        expected: EntitySnapshot,
        item_stack: Option<EntityItemStack>,
    ) -> Result<bool, RegionOwnerLaneError> {
        if let Some(result) =
            self.try_commit_cached_snapshot_mutation(expected.clone(), |expected| {
                RegionOwnerMutation::SetItemStackIfCurrent {
                    expected: Box::new(expected),
                    item_stack: item_stack.clone(),
                }
            })
        {
            return result.map(|snapshots| snapshots.len() == 1);
        }

        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SetItemStackIfCurrent {
                expected: Box::new(expected),
                item_stack,
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    fn try_commit_cached_snapshot_mutation(
        &self,
        expected: EntitySnapshot,
        mutation: impl FnOnce(EntitySnapshot) -> RegionOwnerMutation,
    ) -> Option<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>> {
        let direct_mutation = mutation(expected.clone());
        self.try_commit_cached_snapshot_mutations(vec![(expected, direct_mutation)])
    }

    fn try_commit_cached_snapshot_mutations(
        &self,
        expected_mutations: Vec<(EntitySnapshot, RegionOwnerMutation)>,
    ) -> Option<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>> {
        self.try_commit_cached_snapshot_mutations_with_journal(expected_mutations, true)
    }

    fn try_commit_cached_snapshot_mutations_with_journal(
        &self,
        expected_mutations: Vec<(EntitySnapshot, RegionOwnerMutation)>,
        journal_commit: bool,
    ) -> Option<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>> {
        self.try_commit_cached_snapshot_mutations_inner(
            expected_mutations,
            journal_commit,
            false,
            false,
        )
    }

    fn try_commit_cached_standalone_snapshot_mutations_with_journal(
        &self,
        expected_mutations: Vec<(EntitySnapshot, RegionOwnerMutation)>,
        journal_commit: bool,
    ) -> Option<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>> {
        self.try_commit_cached_snapshot_mutations_inner(
            expected_mutations,
            journal_commit,
            true,
            false,
        )
    }

    fn try_commit_cached_referenced_snapshot_mutations(
        &self,
        expected_mutations: Vec<(EntitySnapshot, RegionOwnerMutation)>,
        expected_version: u64,
    ) -> Option<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>> {
        let _mutation_gate = self
            .mutation_gate
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.active_entity_writers.load(Ordering::Acquire) != 0
            || self.entity_state_version.load(Ordering::Acquire) != expected_version
        {
            #[cfg(test)]
            self.notify_referenced_goal_fallback_probe();
            return None;
        }
        self.try_commit_cached_snapshot_mutations_inner(expected_mutations, true, false, true)
    }

    fn try_commit_cached_snapshot_mutations_inner(
        &self,
        expected_mutations: Vec<(EntitySnapshot, RegionOwnerMutation)>,
        journal_commit: bool,
        require_standalone: bool,
        mutation_gate_held: bool,
    ) -> Option<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>> {
        if expected_mutations.is_empty() {
            return Some(Ok(Vec::new()));
        }
        let _mutation_gate = (!mutation_gate_held).then(|| {
            self.mutation_gate
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        });
        let cached = self
            .selected_read_routes
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut routes = Vec::with_capacity(expected_mutations.len());
        for (expected, _) in &expected_mutations {
            let Some(key) = RegionKey::from_position(expected.position) else {
                return Some(Err(RegionOwnerLaneError::InvalidMutation));
            };
            let route = cached.get(&expected.id)?.clone();
            if route.lease.key != key || route.uuid != expected.uuid {
                return None;
            }
            if require_standalone && !route.standalone {
                return None;
            }
            if routes
                .first()
                .is_some_and(|first: &CachedEntityReadRoute| first.lease.lane != route.lease.lane)
            {
                return None;
            }
            routes.push(route);
        }
        drop(cached);
        let owner = routes[0].owner.clone();
        Some((|| {
            let _lane_admission = owner
                .admission()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let _writer = EntityWriterPermit::enter(
                Arc::clone(&self.active_entity_writers),
                Arc::clone(&self.entity_state_version),
            );
            let (first_sequence, sequence) = self
                .commit_state
                .reserve_sequences(expected_mutations.len())?;
            let phase = self.commit_state.reserve_phase()?;
            let expected_post = expected_mutations
                .iter()
                .zip(&routes)
                .map(|((expected, _), route)| (expected.id, (route.lease, route.uuid)))
                .collect::<BTreeMap<_, _>>();
            if expected_post.len() != expected_mutations.len() {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            let requested = expected_post
                .iter()
                .map(|(&entity, &(lease, _))| (lease, entity))
                .collect::<Vec<_>>();
            let mutations = expected_mutations
                .into_iter()
                .zip(&routes)
                .enumerate()
                .map(|(offset, ((_, mutation), route))| SequencedRegionMutation {
                    sequence: first_sequence + offset as u64 + 1,
                    lease: route.lease,
                    mutation,
                })
                .collect();
            let committed = owner.prepare_and_commit(RegionOwnerBatch {
                phase,
                sequence_watermark: sequence,
                mutations,
            })?;
            match committed.recv().map_err(|_| RegionOwnerLaneError::Closed)? {
                Ok(completion) if completion.phase == phase => {}
                Ok(_) => return Err(RegionOwnerLaneError::StalePhase),
                Err(RegionOwnerLaneError::InvalidMutation) => return Ok(Vec::new()),
                Err(error) => return Err(error),
            }

            let post_state = owner
                .request_existing_snapshots_for_ids(requested)
                .and_then(|snapshots| {
                    snapshots.recv().map_err(|_| RegionOwnerLaneError::Closed)?
                });
            let upserts = match post_state {
                Ok(upserts)
                    if upserts.len() == expected_post.len()
                        && upserts.iter().all(|snapshot| {
                            expected_post
                                .get(&snapshot.id)
                                .is_some_and(|(lease, uuid)| {
                                    snapshot.uuid == *uuid
                                        && RegionKey::from_position(snapshot.position)
                                            == Some(lease.key)
                                })
                        }) =>
                {
                    upserts
                }
                Ok(_) => {
                    owner.rollback_committed(phase)?;
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                Err(error) => {
                    owner.rollback_committed(phase)?;
                    return Err(error);
                }
            };
            let journal_enabled = journal_commit
                && self
                    .commit_state
                    .journal
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .enabled();
            if journal_enabled {
                let decision = RegionalCommitDecision {
                    phase,
                    sequence_watermark: sequence,
                    upserts: upserts.clone(),
                    removed: Vec::new(),
                };
                let journal_result = self.commit_state.record_commit(&decision);
                if let Err(error) = journal_result {
                    if error.outcome_unknown() {
                        self.commit_state
                            .journal_phases
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .insert(phase, sequence);
                        self.commit_state
                            .outcome_unknown
                            .store(true, Ordering::Release);
                        return Err(RegionOwnerLaneError::Journal);
                    }
                    owner.rollback_committed(phase)?;
                    return Err(RegionOwnerLaneError::Journal);
                }
            }
            let finalized = owner.finalize(phase)?;
            match finalized.recv().map_err(|_| RegionOwnerLaneError::Closed)? {
                Ok(finalized) if finalized == phase => Ok(upserts),
                Ok(_) => Err(RegionOwnerLaneError::StalePhase),
                Err(error) => Err(error),
            }
        })())
    }

    pub fn set_position(
        &self,
        entity: EntityId,
        position: Vec3,
    ) -> Result<bool, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SetPosition {
                entity,
                position,
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn set_velocities(
        &self,
        velocities: impl IntoIterator<Item = (EntityId, Vec3)>,
    ) -> Result<(), RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SetVelocities {
                velocities: velocities.into_iter().collect(),
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn apply_kinematics_if_current(
        &self,
        states: impl IntoIterator<Item = (EntitySnapshot, EntityKinematics)>,
    ) -> Result<bool, RegionOwnerLaneError> {
        self.apply_kinematics_if_current_inner(states, true)
    }

    fn apply_kinematics_if_current_inner(
        &self,
        states: impl IntoIterator<Item = (EntitySnapshot, EntityKinematics)>,
        journal_commit: bool,
    ) -> Result<bool, RegionOwnerLaneError> {
        let states = states.into_iter().collect::<Vec<_>>();
        if states.is_empty() {
            return Ok(true);
        }
        let expected_count = states.len();
        let mut ids = HashSet::with_capacity(expected_count);
        let mut direct_eligible = true;
        let mut direct = Vec::with_capacity(expected_count);
        for (expected, state) in &states {
            let source = RegionKey::from_position(expected.position)
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
            if expected.id != state.id || !state.is_finite() || !ids.insert(expected.id) {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            direct_eligible &= RegionKey::from_position(state.position) == Some(source);
            direct.push((
                expected.clone(),
                RegionOwnerMutation::SetKinematicsIfCurrent {
                    expected: Box::new(expected.clone()),
                    state: *state,
                },
            ));
        }
        if direct_eligible
            && let Some(result) = self.try_commit_cached_standalone_snapshot_mutations_with_journal(
                direct,
                journal_commit,
            )
        {
            return result.map(|snapshots| snapshots.len() == expected_count);
        }

        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::ApplyKinematicsIfCurrent {
                states,
                defer_journal: !journal_commit,
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn apply_kinematics_if_current_deferred_journal(
        &self,
        states: impl IntoIterator<Item = (EntitySnapshot, EntityKinematics)>,
    ) -> Result<bool, RegionOwnerLaneError> {
        self.apply_kinematics_if_current_inner(states, false)
    }

    pub fn damage_if_current(
        &self,
        expected: EntitySnapshot,
        amount: f32,
    ) -> Result<Option<EntityDamage>, RegionOwnerLaneError> {
        if !amount.is_finite() {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        if let Some(result) =
            self.try_commit_cached_snapshot_mutation(expected.clone(), |expected| {
                RegionOwnerMutation::DamageIfCurrent {
                    expected: Box::new(expected),
                    amount,
                }
            })
        {
            return match result?.as_slice() {
                [] => Ok(None),
                [snapshot] => Ok(Some(EntityDamage {
                    killed: snapshot.lifecycle == crate::EntityLifecycle::Despawning,
                    snapshot: snapshot.clone(),
                })),
                _ => Err(RegionOwnerLaneError::InvalidMutation),
            };
        }

        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::DamageIfCurrent {
                expected: Box::new(expected),
                amount,
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn damage(
        &self,
        entity: EntityId,
        amount: f32,
    ) -> Result<Option<EntityDamage>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::Damage {
                entity,
                amount,
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn prepare_goal_tick_with_pathing_for_ids(
        &self,
        tick: u64,
        active_ids: &HashSet<EntityId>,
    ) -> Result<RegionalPreparedGoalTick, RegionOwnerLaneError> {
        self.prepare_goal_tick_with_optional_snapshots(tick, active_ids, None)
    }

    pub fn prepare_goal_tick_with_pathing_for_versioned_snapshots(
        &self,
        tick: u64,
        active_ids: &HashSet<EntityId>,
        selected: VersionedEntitySnapshots,
    ) -> Result<RegionalPreparedGoalTick, RegionOwnerLaneError> {
        self.prepare_goal_tick_with_optional_snapshots(tick, active_ids, Some(selected))
    }

    fn prepare_goal_tick_with_optional_snapshots(
        &self,
        tick: u64,
        active_ids: &HashSet<EntityId>,
        selected: Option<VersionedEntitySnapshots>,
    ) -> Result<RegionalPreparedGoalTick, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::PrepareGoalTick {
                tick,
                active_ids: active_ids.clone(),
                selected,
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn apply_prepared_goal_tick(
        &self,
        resolved: RegionalResolvedGoalTick,
    ) -> Result<GoalTickStats, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::ApplyPreparedGoalTick {
                resolved: Box::new(resolved),
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn apply_prepared_goal_tick_and_kinematics_for_ids(
        &self,
        resolved: RegionalResolvedGoalTick,
        entities: &HashSet<EntityId>,
    ) -> Result<Option<(GoalTickStats, Vec<EntityKinematics>)>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::ApplyPreparedGoalTickAndKinematics {
                resolved: Box::new(resolved),
                entities: entities.clone(),
                defer_journal: false,
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    pub fn apply_prepared_goal_tick_and_kinematics_for_ids_deferred_journal(
        &self,
        resolved: RegionalResolvedGoalTick,
        entities: &HashSet<EntityId>,
    ) -> Result<Option<(GoalTickStats, Vec<EntityKinematics>)>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::ApplyPreparedGoalTickAndKinematics {
                resolved: Box::new(resolved),
                entities: entities.clone(),
                defer_journal: true,
                reply,
            })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }

    #[cfg(any(test, feature = "shadow-compare"))]
    pub fn compare_shadow(
        &self,
        tick: u64,
        stage: ShadowStage,
    ) -> Result<Result<ShadowComparison, Box<ShadowDivergence>>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::CompareShadow { tick, stage, reply })
            .map_err(|_| RegionOwnerLaneError::Closed)?;
        result.recv().map_err(|_| RegionOwnerLaneError::Closed)?
    }
}

impl RegionalOwnerRuntime {
    pub fn from_store(
        store: RegionalEntityStore,
        requested_lanes: usize,
    ) -> Result<Self, RegionalOwnerCutoverError> {
        Self::from_store_with_journal(
            store,
            requested_lanes,
            Box::new(NoopRegionalDecisionJournal),
        )
    }

    pub fn from_store_with_journal(
        store: RegionalEntityStore,
        requested_lanes: usize,
        journal: Box<dyn RegionalDecisionJournal>,
    ) -> Result<Self, RegionalOwnerCutoverError> {
        let coordinator =
            RegionalOwnerCoordinator::from_store_with_journal(store, requested_lanes, journal)?;
        let authority = coordinator.authority;
        let commit_state = Arc::clone(&coordinator.commit_state);
        let selected_read_routes = Arc::new(RwLock::new(HashMap::new()));
        let runtime_selected_read_routes = Arc::clone(&selected_read_routes);
        let entity_state_version = Arc::new(AtomicU64::new(0));
        let runtime_entity_state_version = Arc::clone(&entity_state_version);
        let active_entity_writers = Arc::new(AtomicUsize::new(0));
        let runtime_active_entity_writers = Arc::clone(&active_entity_writers);
        let mutation_gate = Arc::new(RwLock::new(()));
        let runtime_mutation_gate = Arc::clone(&mutation_gate);
        let direct_selected_reads = Arc::new(AtomicUsize::new(0));
        #[cfg(test)]
        let selected_read_probe = Arc::new(std::sync::Mutex::new(None));
        #[cfg(test)]
        let referenced_goal_fallback_probe = Arc::new(std::sync::Mutex::new(None));
        let (sender, receiver) = sync_channel(64);
        let (start, started) = channel();
        let worker = match std::thread::Builder::new()
            .name("solaris-region-coordinator".to_owned())
            .spawn(move || {
                run_regional_owner_runtime(
                    started,
                    receiver,
                    runtime_selected_read_routes,
                    runtime_entity_state_version,
                    runtime_active_entity_writers,
                    runtime_mutation_gate,
                );
            }) {
            Ok(worker) => worker,
            Err(_) => {
                return Err(recover_owner_runtime_start(
                    coordinator,
                    RegionOwnerLaneError::SpawnFailed,
                ));
            }
        };
        if let Err(error) = start.send(coordinator) {
            let _ = worker.join();
            return Err(recover_owner_runtime_start(
                error.0,
                RegionOwnerLaneError::Closed,
            ));
        }
        Ok(Self {
            handle: RegionalOwnerHandle {
                sender,
                authority,
                selected_read_routes,
                entity_state_version,
                active_entity_writers,
                mutation_gate,
                direct_selected_reads,
                commit_state,
                #[cfg(test)]
                selected_read_probe,
                #[cfg(test)]
                referenced_goal_fallback_probe,
            },
            worker: Some(worker),
        })
    }

    #[must_use]
    pub fn handle(&self) -> RegionalOwnerHandle {
        self.handle.clone()
    }

    pub fn shutdown(mut self) -> Result<RegionalEntityStore, RegionalOwnerRuntimeShutdownError> {
        self.stop()
    }

    fn stop(&mut self) -> Result<RegionalEntityStore, RegionalOwnerRuntimeShutdownError> {
        let Some(worker) = self.worker.take() else {
            return Err(RegionalOwnerRuntimeShutdownError::Closed);
        };
        let (reply, result) = channel();
        if self
            .handle
            .sender
            .send(RegionalOwnerCommand::Shutdown { reply })
            .is_err()
        {
            return match worker.join() {
                Ok(()) => Err(RegionalOwnerRuntimeShutdownError::Closed),
                Err(_) => Err(RegionalOwnerRuntimeShutdownError::WorkerPanicked),
            };
        }
        let result = result.recv();
        if worker.join().is_err() {
            return Err(RegionalOwnerRuntimeShutdownError::WorkerPanicked);
        }
        result
            .map_err(|_| RegionalOwnerRuntimeShutdownError::Closed)?
            .map_err(RegionalOwnerRuntimeShutdownError::Owner)
    }
}

impl Drop for RegionalOwnerRuntime {
    fn drop(&mut self) {
        if self.worker.is_some() {
            let _ = self.stop();
        }
    }
}

fn run_regional_owner_runtime(
    started: Receiver<RegionalOwnerCoordinator>,
    receiver: Receiver<RegionalOwnerCommand>,
    selected_read_routes: Arc<RwLock<HashMap<EntityId, CachedEntityReadRoute>>>,
    entity_state_version: Arc<AtomicU64>,
    active_entity_writers: Arc<AtomicUsize>,
    mutation_gate: Arc<RwLock<()>>,
) {
    let Ok(mut coordinator) = started.recv() else {
        return;
    };
    while let Ok(command) = receiver.recv() {
        let mutates_entity_snapshots = command.mutates_entity_snapshots();
        let exclusive_lane_access = command.requires_exclusive_lane_access();
        let _mutation_guard = exclusive_lane_access.then(|| {
            mutation_gate
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        });
        if mutates_entity_snapshots {
            active_entity_writers.fetch_add(1, Ordering::AcqRel);
        }
        match command {
            RegionalOwnerCommand::Snapshot { entity, reply } => {
                let result = coordinator.snapshot(entity);
                match &result {
                    Ok(Some(snapshot)) => publish_selected_read_routes(
                        &coordinator,
                        &selected_read_routes,
                        &HashSet::from([entity]),
                        std::slice::from_ref(snapshot),
                    ),
                    Ok(None) => {
                        invalidate_selected_read_routes(&selected_read_routes, [entity]);
                    }
                    Err(_) => {}
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::Snapshots { reply } => {
                let result = coordinator.snapshots();
                if let Ok(snapshots) = &result {
                    let entities = snapshots
                        .iter()
                        .map(|snapshot| snapshot.id)
                        .collect::<HashSet<_>>();
                    publish_selected_read_routes(
                        &coordinator,
                        &selected_read_routes,
                        &entities,
                        snapshots,
                    );
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::SnapshotsForIds { entities, reply } => {
                let result = coordinator.snapshots_for_ids(&entities).map(|snapshots| {
                    VersionedEntitySnapshots {
                        authority: coordinator.authority,
                        version: entity_state_version.load(Ordering::Acquire),
                        snapshots,
                    }
                });
                if let Ok(selected) = &result {
                    publish_selected_read_routes(
                        &coordinator,
                        &selected_read_routes,
                        &entities,
                        selected.snapshots(),
                    );
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::ContainsUuid { uuid, reply } => {
                let _ = reply.send(coordinator.contains_uuid(uuid));
            }
            RegionalOwnerCommand::BreedingTickSnapshots { reply } => {
                let _ = reply.send(coordinator.breeding_tick_snapshots());
            }
            RegionalOwnerCommand::Status { reply } => {
                let _ = reply.send(coordinator.status());
            }
            RegionalOwnerCommand::ReconfigureLanes { lane_count, reply } => {
                let result = coordinator.reconfigure_lanes(lane_count);
                if result.is_ok() {
                    selected_read_routes
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clear();
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::SaveBarrier { reply } => {
                let _ = reply.send(coordinator.save_barrier());
            }
            RegionalOwnerCommand::SpawnAuthoritative {
                entity,
                defer_journal,
                reply,
            } => {
                let passenger = entity_vehicle_reference(&entity);
                let result = coordinator.spawn_authoritative_inner(*entity, !defer_journal);
                if result.is_ok() {
                    invalidate_selected_read_routes(&selected_read_routes, passenger.into_iter());
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::SpawnAuthoritativeBatch { entities, reply } => {
                let passengers = entities
                    .iter()
                    .filter_map(entity_vehicle_reference)
                    .collect::<Vec<_>>();
                let result = coordinator.spawn_authoritative_batch(entities);
                if result.is_ok() {
                    invalidate_selected_read_routes(&selected_read_routes, passengers);
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::SpawnUniqueAuthoritativeBatch { entities, reply } => {
                let result = coordinator.spawn_unique_authoritative_batch(entities);
                if let Ok(snapshots) = &result {
                    invalidate_selected_read_routes(
                        &selected_read_routes,
                        snapshots.iter().filter_map(snapshot_vehicle_reference),
                    );
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::InsertAuthoritativeSnapshots { snapshots, reply } => {
                let passengers = snapshots
                    .iter()
                    .filter_map(snapshot_vehicle_reference)
                    .collect::<Vec<_>>();
                let result = coordinator.insert_authoritative_snapshots_batch(snapshots);
                if result.is_ok() {
                    invalidate_selected_read_routes(&selected_read_routes, passengers);
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::Remove { entity, reply } => {
                let result = coordinator.remove(entity);
                if let Ok(Some(snapshot)) = &result {
                    invalidate_selected_read_routes(
                        &selected_read_routes,
                        std::iter::once(entity).chain(snapshot_vehicle_reference(snapshot)),
                    );
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::RemoveIfCurrent { expected, reply } => {
                let entity = expected.id;
                let passenger = snapshot_vehicle_reference(&expected);
                let result = coordinator.remove_if_current(*expected);
                if matches!(&result, Ok(Some(_))) {
                    invalidate_selected_read_routes(
                        &selected_read_routes,
                        std::iter::once(entity).chain(passenger),
                    );
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::SetAnimalStatesIfCurrent {
                states,
                defer_journal,
                reply,
            } => {
                let _ = reply
                    .send(coordinator.set_animal_states_if_current_inner(states, !defer_journal));
            }
            RegionalOwnerCommand::SetGoal {
                entity,
                goal,
                reply,
            } => {
                let _ = reply.send(coordinator.set_goal(entity, goal));
            }
            RegionalOwnerCommand::SetGoals {
                goals,
                defer_journal,
                reply,
            } => {
                let _ = reply.send(coordinator.set_goals_inner(goals, !defer_journal));
            }
            RegionalOwnerCommand::SetItemStack {
                entity,
                item_stack,
                reply,
            } => {
                let _ = reply.send(coordinator.set_item_stack(entity, item_stack));
            }
            RegionalOwnerCommand::SetItemStackIfCurrent {
                expected,
                item_stack,
                reply,
            } => {
                let _ = reply.send(coordinator.set_item_stack_if_current(*expected, item_stack));
            }
            RegionalOwnerCommand::SetPosition {
                entity,
                position,
                reply,
            } => {
                let old_region = coordinator.locations.get(&entity).copied();
                let result = coordinator.set_position(entity, position);
                if matches!(&result, Ok(true)) && old_region != RegionKey::from_position(position) {
                    invalidate_selected_read_routes(&selected_read_routes, [entity]);
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::SetVelocities { velocities, reply } => {
                let _ = reply.send(coordinator.set_velocities(velocities));
            }
            RegionalOwnerCommand::ApplyKinematicsIfCurrent {
                states,
                defer_journal,
                reply,
            } => {
                let crosses_region = states.iter().any(|(expected, state)| {
                    RegionKey::from_position(expected.position)
                        != RegionKey::from_position(state.position)
                });
                let result = coordinator.apply_kinematics_if_current_inner(states, !defer_journal);
                if matches!(&result, Ok(true)) && crosses_region {
                    selected_read_routes
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clear();
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::DamageIfCurrent {
                expected,
                amount,
                reply,
            } => {
                let _ = reply.send(coordinator.damage_if_current(*expected, amount));
            }
            RegionalOwnerCommand::Damage {
                entity,
                amount,
                reply,
            } => {
                let _ = reply.send(coordinator.damage(entity, amount));
            }
            RegionalOwnerCommand::PrepareGoalTick {
                tick,
                active_ids,
                selected,
                reply,
            } => {
                let selected = selected
                    .filter(|selected| {
                        selected.authority == coordinator.authority
                            && selected.version == entity_state_version.load(Ordering::Acquire)
                    })
                    .map(VersionedEntitySnapshots::into_snapshots);
                let _ = reply.send(
                    coordinator.prepare_goal_tick_with_pathing_for_ids_from_snapshots(
                        tick,
                        &active_ids,
                        selected.as_deref(),
                    ),
                );
            }
            RegionalOwnerCommand::ApplyPreparedGoalTick { resolved, reply } => {
                let entities = resolved.goal_inputs.keys().copied().collect::<Vec<_>>();
                let result = coordinator.apply_prepared_goal_tick(*resolved);
                if result.is_ok() {
                    invalidate_selected_read_routes(&selected_read_routes, entities);
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::ApplyPreparedGoalTickAndKinematics {
                resolved,
                entities,
                defer_journal,
                reply,
            } => {
                let result = coordinator.apply_prepared_goal_tick_and_kinematics_for_ids(
                    *resolved,
                    &entities,
                    !defer_journal,
                );
                if result.is_ok() {
                    invalidate_selected_read_routes(&selected_read_routes, entities);
                }
                let _ = reply.send(result);
            }
            #[cfg(any(test, feature = "shadow-compare"))]
            RegionalOwnerCommand::CompareShadow { tick, stage, reply } => {
                let _ = reply.send(coordinator.compare_shadow(tick, stage));
            }
            #[cfg(test)]
            RegionalOwnerCommand::HoldForTest { entered, release } => {
                let _ = entered.send(());
                let _ = release.recv();
            }
            RegionalOwnerCommand::Shutdown { reply } => {
                let _ = reply.send(coordinator.shutdown());
                return;
            }
        }
        if mutates_entity_snapshots {
            entity_state_version.fetch_add(1, Ordering::Release);
            active_entity_writers.fetch_sub(1, Ordering::Release);
        }
    }
    let _ = coordinator.shutdown();
}

fn publish_selected_read_routes(
    coordinator: &RegionalOwnerCoordinator,
    routes: &RwLock<HashMap<EntityId, CachedEntityReadRoute>>,
    requested: &HashSet<EntityId>,
    snapshots: &[EntitySnapshot],
) {
    let mut refreshed = Vec::with_capacity(snapshots.len());
    for snapshot in snapshots {
        let Some(key) = coordinator.locations.get(&snapshot.id).copied() else {
            return;
        };
        let Some(lease) = coordinator.ownership.lease(key) else {
            return;
        };
        let Some(owner) = coordinator.lanes.get(&lease.lane) else {
            return;
        };
        if coordinator.uuids.get(&snapshot.uuid).copied() != Some(snapshot.id)
            || RegionKey::from_position(snapshot.position) != Some(key)
        {
            return;
        }
        refreshed.push((
            snapshot.id,
            CachedEntityReadRoute {
                lease,
                uuid: snapshot.uuid,
                standalone: !coordinator.vehicle_passengers.contains_key(&snapshot.id)
                    && !coordinator.passenger_vehicles.contains_key(&snapshot.id),
                owner: owner.reader(),
            },
        ));
    }

    let mut routes = routes
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for entity in requested {
        routes.remove(entity);
    }
    routes.extend(refreshed);
}

fn invalidate_selected_read_routes(
    routes: &RwLock<HashMap<EntityId, CachedEntityReadRoute>>,
    entities: impl IntoIterator<Item = EntityId>,
) {
    let mut routes = routes
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for entity in entities {
        routes.remove(&entity);
    }
}

fn recover_owner_runtime_start(
    coordinator: RegionalOwnerCoordinator,
    error: RegionOwnerLaneError,
) -> RegionalOwnerCutoverError {
    match coordinator.shutdown() {
        Ok(store) => RegionalOwnerCutoverError {
            error,
            store: Box::new(store),
        },
        Err(shutdown) => RegionalOwnerCutoverError {
            error: shutdown.error,
            store: shutdown.recovered,
        },
    }
}

enum OwnerKinematicsPlan {
    Local {
        source: RegionKey,
        expected: Box<EntitySnapshot>,
        state: EntityKinematics,
    },
    Migrate {
        source: RegionKey,
        target: RegionKey,
        expected: Vec<EntitySnapshot>,
        moved: Vec<EntitySnapshot>,
    },
}

impl RegionalOwnerCoordinator {
    pub fn from_store(
        regions: RegionalEntityStore,
        requested_lanes: usize,
    ) -> Result<Self, RegionalOwnerCutoverError> {
        Self::from_store_with_journal(
            regions,
            requested_lanes,
            Box::new(NoopRegionalDecisionJournal),
        )
    }

    pub fn from_store_with_journal(
        regions: RegionalEntityStore,
        requested_lanes: usize,
        journal: Box<dyn RegionalDecisionJournal>,
    ) -> Result<Self, RegionalOwnerCutoverError> {
        if requested_lanes == 0 {
            return Err(RegionalOwnerCutoverError {
                error: RegionOwnerLaneError::InvalidLaneCount,
                store: Box::new(regions),
            });
        }
        if regions.ownership.active_phase.is_some()
            || !regions.transfers.is_empty()
            || !regions.in_flight_transfers.is_empty()
        {
            return Err(RegionalOwnerCutoverError {
                error: RegionOwnerLaneError::Busy,
                store: Box::new(regions),
            });
        }
        let vehicle_links = regions
            .stores
            .values()
            .flat_map(EntityStore::snapshots)
            .filter_map(|snapshot| {
                snapshot_vehicle_reference(&snapshot).map(|passenger| (snapshot.id, passenger))
            })
            .collect::<Vec<_>>();
        let mut vehicle_passengers = HashMap::new();
        let mut passenger_vehicles = HashMap::new();
        for (vehicle, passenger) in vehicle_links {
            if !regions.locations.contains_key(&passenger)
                || vehicle_passengers.insert(vehicle, passenger).is_some()
                || passenger_vehicles.insert(passenger, vehicle).is_some()
            {
                return Err(RegionalOwnerCutoverError {
                    error: RegionOwnerLaneError::InvalidMutation,
                    store: Box::new(regions),
                });
            }
        }
        let RegionalEntityStore {
            authority,
            mut ownership,
            mut stores,
            mut locations,
            mut uuids,
            transfers,
            in_flight_transfers,
            #[cfg(any(test, feature = "shadow-compare"))]
            shadow_stats,
            next_id,
        } = regions;
        let (recovered_phase, recovered_sequence) = journal.recovery_watermark();
        ownership.last_phase = ownership.last_phase.max(recovered_phase.0);
        let lane_count = requested_lanes;
        let mut by_lane = (0..lane_count)
            .map(|lane| (lane, Vec::new()))
            .collect::<BTreeMap<_, _>>();
        let keys = stores.keys().copied().collect::<Vec<_>>();
        for (index, key) in keys.into_iter().enumerate() {
            let target_lane = index % lane_count;
            let current = ownership.lease(key).expect("owned store has a lease");
            let lease = if current.lane == target_lane {
                current
            } else {
                ownership
                    .reassign(current, target_lane)
                    .expect("inactive owner cutover reassigns exact lease")
            };
            let store = stores.remove(&key).expect("known owner cutover store");
            by_lane
                .get_mut(&target_lane)
                .expect("known owner lane")
                .push((lease, store));
        }
        let mut lanes = BTreeMap::new();
        while let Some((lane, stores)) = by_lane.pop_first() {
            match RegionalOwnerLane::spawn_after(lane, stores, recovered_phase, recovered_sequence)
            {
                Ok(owner) => {
                    lanes.insert(lane, owner);
                }
                Err(start) => {
                    let mut recovered = start
                        .regions
                        .into_iter()
                        .map(|(lease, store)| (lease.key, store))
                        .collect::<BTreeMap<_, _>>();
                    for (_, pending) in by_lane {
                        recovered
                            .extend(pending.into_iter().map(|(lease, store)| (lease.key, store)));
                    }
                    let mut recovery_error = start.error;
                    for (_, owner) in lanes {
                        match owner.shutdown() {
                            Ok(stores) => recovered.extend(stores),
                            Err(error) => recovery_error = error,
                        }
                    }
                    retain_recovered_regional_state(
                        &mut ownership,
                        &recovered,
                        &mut locations,
                        &mut uuids,
                    );
                    return Err(RegionalOwnerCutoverError {
                        error: recovery_error,
                        store: Box::new(RegionalEntityStore {
                            authority,
                            ownership,
                            stores: recovered,
                            locations,
                            uuids,
                            transfers,
                            in_flight_transfers,
                            #[cfg(any(test, feature = "shadow-compare"))]
                            shadow_stats,
                            next_id,
                        }),
                    });
                }
            }
        }
        let journal_phases = journal
            .pending_phases()
            .into_iter()
            .map(|phase| (phase, recovered_sequence))
            .collect::<BTreeMap<_, _>>();
        let commit_state = Arc::new(RegionalOwnerCommitState {
            next_phase: AtomicU64::new(ownership.last_phase.max(recovered_phase.0)),
            next_sequence: AtomicU64::new(recovered_sequence),
            journal: Mutex::new(journal),
            journal_phases: Mutex::new(journal_phases),
            outcome_unknown: AtomicBool::new(false),
            #[cfg(test)]
            save_barrier_phase_snapshot_hook: Mutex::new(None),
        });
        Ok(Self {
            authority,
            ownership,
            locations,
            uuids,
            vehicle_passengers,
            passenger_vehicles,
            transfers,
            in_flight_transfers,
            #[cfg(any(test, feature = "shadow-compare"))]
            shadow_stats,
            next_id,
            lanes,
            commit_state,
        })
    }

    #[must_use]
    pub fn lane_count(&self) -> usize {
        self.lanes.len()
    }

    pub fn reconfigure_lanes(
        &mut self,
        requested_lanes: usize,
    ) -> Result<usize, RegionOwnerLaneError> {
        if requested_lanes == 0 {
            return Err(RegionOwnerLaneError::InvalidLaneCount);
        }

        for lane in 0..requested_lanes {
            if self.lanes.contains_key(&lane) {
                continue;
            }
            let owner = RegionalOwnerLane::spawn_after(
                lane,
                [],
                RegionPhase(self.commit_state.next_phase.load(Ordering::Acquire)),
                self.commit_state.sequence_watermark(),
            )
            .map_err(|start| start.error)?;
            self.lanes.insert(lane, owner);
        }

        let leases = self.ownership.leases().collect::<Vec<_>>();
        for (index, lease) in leases.into_iter().enumerate() {
            let target_lane = index % requested_lanes;
            if lease.lane != target_lane {
                self.move_region(lease, target_lane)?;
            }
        }

        let retiring = self
            .lanes
            .range(requested_lanes..)
            .map(|(&lane, _)| lane)
            .collect::<Vec<_>>();
        for lane in retiring {
            let owner = self
                .lanes
                .remove(&lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?;
            let stores = owner.shutdown()?;
            if !stores.is_empty() {
                let regions = stores
                    .into_iter()
                    .map(|(key, store)| {
                        let lease = self
                            .ownership
                            .lease(key)
                            .expect("retiring owner region retains its lease");
                        (lease, store)
                    })
                    .collect::<Vec<_>>();
                let restored = RegionalOwnerLane::spawn_after(
                    lane,
                    regions,
                    RegionPhase(self.commit_state.next_phase.load(Ordering::Acquire)),
                    self.commit_state.sequence_watermark(),
                )
                .map_err(|start| start.error)?;
                self.lanes.insert(lane, restored);
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
        }
        Ok(self.lanes.len())
    }

    pub fn clear_recovered_commits(
        &mut self,
        phases: impl IntoIterator<Item = RegionPhase>,
    ) -> Result<(), RegionOwnerLaneError> {
        let phases = phases.into_iter().collect::<Vec<_>>();
        self.commit_state.clear_recovered_commits(&phases)
    }

    fn move_region(
        &mut self,
        expected: RegionLease,
        target_lane: usize,
    ) -> Result<RegionLease, RegionOwnerLaneError> {
        if !self.lanes.contains_key(&target_lane) {
            return Err(RegionOwnerLaneError::WrongLane);
        }
        let (_, store) = self
            .lanes
            .get(&expected.lane)
            .ok_or(RegionOwnerLaneError::WrongLane)?
            .detach_region(expected)?;
        let reassigned = match self.ownership.reassign(expected, target_lane) {
            Ok(reassigned) => reassigned,
            Err(_) => {
                self.lanes
                    .get(&expected.lane)
                    .expect("source lane was validated before detach")
                    .install_region(expected, store)
                    .map_err(|(rollback, _)| rollback)?;
                return Err(RegionOwnerLaneError::Busy);
            }
        };
        let installed = self
            .lanes
            .get(&target_lane)
            .ok_or(RegionOwnerLaneError::WrongLane)?
            .install_region(reassigned, store);
        if let Err((error, store)) = installed {
            let restored = self
                .ownership
                .reassign(reassigned, expected.lane)
                .map_err(|_| RegionOwnerLaneError::InvalidMutation)?;
            self.lanes
                .get(&expected.lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?
                .install_region(restored, *store)
                .map_err(|(rollback, _)| rollback)?;
            return Err(error);
        }
        Ok(reassigned)
    }

    pub fn snapshot(
        &self,
        entity: EntityId,
    ) -> Result<Option<EntitySnapshot>, RegionOwnerLaneError> {
        let Some(key) = self.locations.get(&entity).copied() else {
            return Ok(None);
        };
        let lease = self
            .ownership
            .lease(key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        self.lanes
            .get(&lease.lane)
            .ok_or(RegionOwnerLaneError::WrongLane)?
            .snapshot(lease, entity)
    }

    pub fn snapshots(&self) -> Result<Vec<EntitySnapshot>, RegionOwnerLaneError> {
        let mut pending = Vec::with_capacity(self.lanes.len());
        for owner in self.lanes.values() {
            pending.push(owner.request_snapshots()?);
        }
        let mut snapshots = Vec::with_capacity(self.locations.len());
        for completion in pending {
            snapshots.extend(
                completion
                    .recv()
                    .map_err(|_| RegionOwnerLaneError::Closed)?,
            );
        }
        snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
        if snapshots.len() != self.locations.len()
            || snapshots.iter().any(|snapshot| {
                self.locations.get(&snapshot.id).copied()
                    != RegionKey::from_position(snapshot.position)
                    || self.uuids.get(&snapshot.uuid).copied() != Some(snapshot.id)
            })
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        Ok(snapshots)
    }

    fn alive_kinematics_for_ids(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<Vec<EntityKinematics>, RegionOwnerLaneError> {
        let mut ordered = entities.iter().copied().collect::<Vec<_>>();
        ordered.sort_unstable();
        let mut requests = BTreeMap::<usize, Vec<(RegionLease, EntityId)>>::new();
        for entity in ordered {
            let Some(expected_key) = self.locations.get(&entity).copied() else {
                continue;
            };
            let lease = self
                .ownership
                .lease(expected_key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            requests
                .entry(lease.lane)
                .or_default()
                .push((lease, entity));
        }
        let mut pending = Vec::with_capacity(requests.len());
        for (lane, entities) in requests {
            let owner = self
                .lanes
                .get(&lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?;
            pending.push(owner.reader().request_alive_kinematics_for_ids(entities)?);
        }
        let mut states = Vec::with_capacity(entities.len());
        for completion in pending {
            states.extend(
                completion
                    .recv()
                    .map_err(|_| RegionOwnerLaneError::Closed)??,
            );
        }
        states.sort_unstable_by_key(|state| state.id);
        if states.iter().any(|state| {
            !entities.contains(&state.id)
                || self.locations.get(&state.id).copied()
                    != RegionKey::from_position(state.position)
        }) {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        Ok(states)
    }

    #[must_use]
    pub fn contains_uuid(&self, uuid: Uuid) -> bool {
        self.uuids.contains_key(&uuid)
    }

    pub fn breeding_tick_snapshots(&self) -> Result<Vec<EntitySnapshot>, RegionOwnerLaneError> {
        let mut pending = Vec::with_capacity(self.lanes.len());
        for owner in self.lanes.values() {
            pending.push(owner.request_breeding_tick_snapshots()?);
        }
        let mut snapshots = Vec::new();
        for completion in pending {
            snapshots.extend(
                completion
                    .recv()
                    .map_err(|_| RegionOwnerLaneError::Closed)?,
            );
        }
        snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
        if snapshots.iter().any(|snapshot| {
            self.locations.get(&snapshot.id).copied() != RegionKey::from_position(snapshot.position)
                || self.uuids.get(&snapshot.uuid).copied() != Some(snapshot.id)
        }) {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        Ok(snapshots)
    }

    pub fn snapshots_for_ids(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<Vec<EntitySnapshot>, RegionOwnerLaneError> {
        let mut ordered = entities.iter().copied().collect::<Vec<_>>();
        ordered.sort_unstable();
        let mut requests = BTreeMap::<usize, Vec<(RegionLease, EntityId)>>::new();
        for entity in ordered {
            let Some(expected_key) = self.locations.get(&entity).copied() else {
                continue;
            };
            let lease = self
                .ownership
                .lease(expected_key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            requests
                .entry(lease.lane)
                .or_default()
                .push((lease, entity));
        }
        let mut pending = Vec::with_capacity(requests.len());
        for (lane, entities) in requests {
            let owner = self
                .lanes
                .get(&lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?;
            pending.push(owner.request_snapshots_for_ids(entities)?);
        }
        let mut snapshots = Vec::with_capacity(entities.len());
        for completion in pending {
            snapshots.extend(
                completion
                    .recv()
                    .map_err(|_| RegionOwnerLaneError::Closed)??,
            );
        }
        snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
        for snapshot in &snapshots {
            let expected_key = self
                .locations
                .get(&snapshot.id)
                .copied()
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
            if RegionKey::from_position(snapshot.position) != Some(expected_key)
                || self.uuids.get(&snapshot.uuid).copied() != Some(snapshot.id)
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
        }
        Ok(snapshots)
    }

    #[must_use]
    pub fn status(&self) -> RegionalOwnerStatus {
        RegionalOwnerStatus {
            entity_count: self.locations.len(),
            lane_count: self.lanes.len(),
            #[cfg(any(test, feature = "shadow-compare"))]
            shadow: self.shadow_stats.clone(),
        }
    }

    #[cfg(any(test, feature = "shadow-compare"))]
    pub fn compare_shadow(
        &mut self,
        tick: u64,
        stage: ShadowStage,
    ) -> Result<Result<ShadowComparison, Box<ShadowDivergence>>, RegionOwnerLaneError> {
        let mut pending = Vec::with_capacity(self.lanes.len());
        for owner in self.lanes.values() {
            pending.push(owner.request_shadow_comparison(tick, stage)?);
        }
        let mut combined = RegionalShadowComparisonBatch::new(tick, stage);
        for completion in pending {
            let batch = completion
                .recv()
                .map_err(|_| RegionOwnerLaneError::Closed)??;
            combined
                .merge(batch)
                .map_err(|_| RegionOwnerLaneError::InvalidMutation)?;
        }
        let outcome = combined.finish();
        Ok(record_regional_shadow_comparison(
            &mut self.shadow_stats,
            outcome,
        ))
    }

    pub fn save_barrier(&self) -> Result<RegionalOwnerSaveSnapshot, RegionOwnerLaneError> {
        let sequence_watermark = self.commit_state.sequence_watermark();
        let mut leases_by_lane = BTreeMap::<usize, Vec<RegionLease>>::new();
        for lease in self.ownership.leases() {
            leases_by_lane.entry(lease.lane).or_default().push(lease);
        }
        let mut pending = Vec::with_capacity(leases_by_lane.len());
        for (lane, leases) in leases_by_lane {
            let completion = self
                .lanes
                .get(&lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?
                .request_save_barrier(sequence_watermark, leases)?;
            pending.push(completion);
        }
        let mut snapshots = Vec::with_capacity(self.locations.len());
        for completion in pending {
            snapshots.extend(
                completion
                    .recv()
                    .map_err(|_| RegionOwnerLaneError::Closed)??,
            );
        }
        snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
        if snapshots.len() != self.locations.len()
            || snapshots.iter().any(|snapshot| {
                self.locations.get(&snapshot.id).copied()
                    != RegionKey::from_position(snapshot.position)
                    || self.uuids.get(&snapshot.uuid).copied() != Some(snapshot.id)
            })
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        Ok(RegionalOwnerSaveSnapshot {
            sequence_watermark,
            snapshots,
            journal_phases: self.commit_state.pending_journal_phases(sequence_watermark),
        })
    }

    pub fn insert_authoritative_snapshots_batch(
        &mut self,
        snapshots: impl IntoIterator<Item = EntitySnapshot>,
    ) -> Result<usize, RegionOwnerLaneError> {
        let snapshots = snapshots.into_iter().collect::<Vec<_>>();
        if snapshots.is_empty() {
            return Ok(0);
        }
        let mut pending_ids = HashSet::with_capacity(snapshots.len());
        let mut pending_uuids = HashSet::with_capacity(snapshots.len());
        let mut locations = self.locations.clone();
        for snapshot in &snapshots {
            let key = RegionKey::from_position(snapshot.position)
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
            if !snapshot.rotation.is_finite()
                || !snapshot.velocity.is_finite()
                || self.locations.contains_key(&snapshot.id)
                || self.uuids.contains_key(&snapshot.uuid)
                || !pending_ids.insert(snapshot.id)
                || !pending_uuids.insert(snapshot.uuid)
                || locations.insert(snapshot.id, key).is_some()
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
        }
        if snapshots.iter().any(|snapshot| {
            let key = locations[&snapshot.id];
            snapshot_vehicle_reference(snapshot)
                .is_some_and(|passenger| locations.get(&passenger).copied() != Some(key))
                || goal_reference(&snapshot.goal)
                    .is_some_and(|target| !locations.contains_key(&target))
        }) {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        if snapshots
            .iter()
            .any(|snapshot| snapshot_vehicle_reference(snapshot).is_some())
        {
            let current = self.snapshots()?;
            let mut graph_validator = EntityStore::new();
            if !graph_validator
                .insert_authoritative_snapshots_batch(current.into_iter().chain(snapshots.clone()))
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
        }

        let mut grouped = BTreeMap::<RegionKey, Vec<EntitySnapshot>>::new();
        for snapshot in &snapshots {
            let key = RegionKey::from_position(snapshot.position)
                .expect("validated owner restore position");
            self.ensure_region(key)?;
            grouped.entry(key).or_default().push(snapshot.clone());
        }
        let (first_sequence, next_sequence) = self.commit_state.reserve_sequences(grouped.len())?;
        let mut mutations = BTreeMap::<usize, Vec<SequencedRegionMutation>>::new();
        for (offset, (key, snapshots)) in grouped.into_iter().enumerate() {
            let lease = self
                .ownership
                .lease(key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            mutations
                .entry(lease.lane)
                .or_default()
                .push(SequencedRegionMutation {
                    sequence: first_sequence + offset as u64 + 1,
                    lease,
                    mutation: RegionOwnerMutation::InsertSnapshots(snapshots),
                });
        }
        self.execute_mutations(mutations, next_sequence)?;
        for snapshot in &snapshots {
            let key = RegionKey::from_position(snapshot.position)
                .expect("validated owner restore position");
            self.next_id = self.next_id.max(snapshot.id.0);
            self.locations.insert(snapshot.id, key);
            self.uuids.insert(snapshot.uuid, snapshot.id);
            self.index_vehicle_link(snapshot);
        }
        Ok(snapshots.len())
    }

    pub fn spawn_authoritative(
        &mut self,
        entity: SpawnEntity,
    ) -> Result<EntityId, RegionOwnerLaneError> {
        self.spawn_authoritative_inner(entity, true)
    }

    fn spawn_authoritative_inner(
        &mut self,
        entity: SpawnEntity,
        journal_commit: bool,
    ) -> Result<EntityId, RegionOwnerLaneError> {
        let key = RegionKey::from_position(entity.position)
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        if !entity.rotation.is_finite()
            || !entity.velocity.is_finite()
            || entity
                .vehicle
                .as_ref()
                .and_then(|vehicle| vehicle.passenger)
                .is_some()
            || goal_reference(&entity.goal).is_some()
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        let lease = self.ensure_region(key)?;
        let id = self.next_owner_id()?;
        let uuid = entity.uuid.unwrap_or_else(|| deterministic_uuid(id));
        if self.uuids.contains_key(&uuid) {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        let snapshot = snapshot_from_spawn(id, uuid, entity);
        let (_, sequence) = self.commit_state.reserve_sequences(1)?;
        let mutations = BTreeMap::from([(
            lease.lane,
            vec![SequencedRegionMutation {
                sequence,
                lease,
                mutation: RegionOwnerMutation::InsertSnapshot(Box::new(snapshot)),
            }],
        )]);
        self.execute_mutations_with_stats(mutations, sequence, journal_commit)?;
        self.next_id = id.0;
        self.locations.insert(id, key);
        self.uuids.insert(uuid, id);
        Ok(id)
    }

    pub fn spawn_authoritative_batch(
        &mut self,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Result<Vec<EntityId>, RegionOwnerLaneError> {
        let entities = entities.into_iter().collect::<Vec<_>>();
        let mut snapshots = Vec::with_capacity(entities.len());
        let mut ids = Vec::with_capacity(entities.len());
        let mut pending_ids = HashSet::with_capacity(entities.len());
        let mut pending_uuids = HashSet::with_capacity(entities.len());
        let mut cursor = self.next_id;
        for entity in entities {
            if RegionKey::from_position(entity.position).is_none()
                || !entity.rotation.is_finite()
                || !entity.velocity.is_finite()
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            let id = loop {
                cursor = cursor
                    .max(0)
                    .checked_add(1)
                    .ok_or(RegionOwnerLaneError::InvalidMutation)?;
                let id = EntityId(cursor);
                if !self.locations.contains_key(&id) && pending_ids.insert(id) {
                    break id;
                }
            };
            let uuid = entity.uuid.unwrap_or_else(|| deterministic_uuid(id));
            if self.uuids.contains_key(&uuid) || !pending_uuids.insert(uuid) {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            ids.push(id);
            snapshots.push(snapshot_from_spawn(id, uuid, entity));
        }
        let expected = snapshots.len();
        if self.insert_authoritative_snapshots_batch(snapshots)? != expected {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        Ok(ids)
    }

    pub fn spawn_unique_authoritative_batch(
        &mut self,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Result<Vec<EntitySnapshot>, RegionOwnerLaneError> {
        let entities = entities.into_iter().collect::<Vec<_>>();
        let mut snapshots = Vec::with_capacity(entities.len());
        let mut pending_ids = HashSet::with_capacity(entities.len());
        let mut pending_uuids = HashSet::with_capacity(entities.len());
        let mut cursor = self.next_id;
        for entity in entities {
            if let Some(uuid) = entity.uuid
                && (self.uuids.contains_key(&uuid) || !pending_uuids.insert(uuid))
            {
                continue;
            }
            if RegionKey::from_position(entity.position).is_none()
                || !entity.rotation.is_finite()
                || !entity.velocity.is_finite()
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            let id = loop {
                cursor = cursor
                    .max(0)
                    .checked_add(1)
                    .ok_or(RegionOwnerLaneError::InvalidMutation)?;
                let id = EntityId(cursor);
                if !self.locations.contains_key(&id) && pending_ids.insert(id) {
                    break id;
                }
            };
            let uuid = if let Some(uuid) = entity.uuid {
                uuid
            } else {
                let uuid = deterministic_uuid(id);
                if self.uuids.contains_key(&uuid) || !pending_uuids.insert(uuid) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                uuid
            };
            snapshots.push(snapshot_from_spawn(id, uuid, entity));
        }
        if snapshots.is_empty() {
            return Ok(Vec::new());
        }
        let expected = snapshots.len();
        if self.insert_authoritative_snapshots_batch(snapshots.clone())? != expected {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        Ok(snapshots)
    }

    pub fn remove(
        &mut self,
        entity: EntityId,
    ) -> Result<Option<EntitySnapshot>, RegionOwnerLaneError> {
        let Some(key) = self.locations.get(&entity).copied() else {
            return Ok(None);
        };
        if self.in_flight_transfers.contains_key(&entity) {
            return Err(RegionOwnerLaneError::Busy);
        }
        let lease = self
            .ownership
            .lease(key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let snapshot = self
            .lanes
            .get(&lease.lane)
            .ok_or(RegionOwnerLaneError::WrongLane)?
            .snapshot(lease, entity)?
            .ok_or(RegionOwnerLaneError::UnknownEntity)?;
        let (_, sequence) = self.commit_state.reserve_sequences(1)?;
        let mutations = BTreeMap::from([(
            lease.lane,
            vec![SequencedRegionMutation {
                sequence,
                lease,
                mutation: RegionOwnerMutation::RemoveEntity(entity),
            }],
        )]);
        self.execute_mutations(mutations, sequence)?;
        self.remove_vehicle_links(&snapshot);
        self.locations.remove(&entity);
        self.uuids.remove(&snapshot.uuid);
        Ok(Some(snapshot))
    }

    pub fn remove_if_current(
        &mut self,
        expected: EntitySnapshot,
    ) -> Result<Option<EntitySnapshot>, RegionOwnerLaneError> {
        let Some(key) = RegionKey::from_position(expected.position) else {
            return Err(RegionOwnerLaneError::InvalidMutation);
        };
        if self.locations.get(&expected.id).copied() != Some(key) {
            return Ok(None);
        }
        if self.in_flight_transfers.contains_key(&expected.id) {
            return Err(RegionOwnerLaneError::Busy);
        }
        if self.snapshot(expected.id)?.as_ref() != Some(&expected) {
            return Ok(None);
        }
        let lease = self
            .ownership
            .lease(key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let (_, sequence) = self.commit_state.reserve_sequences(1)?;
        let entity = expected.id;
        let uuid = expected.uuid;
        let mutations = BTreeMap::from([(
            lease.lane,
            vec![SequencedRegionMutation {
                sequence,
                lease,
                mutation: RegionOwnerMutation::RemoveIfCurrent(Box::new(expected.clone())),
            }],
        )]);
        match self.execute_mutations(mutations, sequence) {
            Ok(()) => {
                self.remove_vehicle_links(&expected);
                self.locations.remove(&entity);
                self.uuids.remove(&uuid);
                Ok(Some(expected))
            }
            Err(error) => Err(error),
        }
    }

    fn ensure_region(&mut self, key: RegionKey) -> Result<RegionLease, RegionOwnerLaneError> {
        if let Some(lease) = self.ownership.lease(key) {
            return Ok(lease);
        }
        let mut region_counts = self
            .lanes
            .keys()
            .map(|lane| (*lane, 0usize))
            .collect::<BTreeMap<_, _>>();
        for lease in self.ownership.leases() {
            let count = region_counts
                .get_mut(&lease.lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?;
            *count += 1;
        }
        let lane = region_counts
            .into_iter()
            .min_by_key(|(lane, count)| (*count, *lane))
            .map(|(lane, _)| lane)
            .ok_or(RegionOwnerLaneError::InvalidLaneCount)?;
        let lease = self
            .ownership
            .assign(key, lane)
            .map_err(|_| RegionOwnerLaneError::Busy)?;
        let installed = self
            .lanes
            .get(&lane)
            .ok_or(RegionOwnerLaneError::WrongLane)?
            .install_region(lease, EntityStore::new());
        if let Err((error, _)) = installed {
            self.ownership
                .unassign(lease)
                .map_err(|_| RegionOwnerLaneError::Busy)?;
            return Err(error);
        }
        Ok(lease)
    }

    fn next_owner_id(&self) -> Result<EntityId, RegionOwnerLaneError> {
        let mut next = self.next_id.max(0);
        loop {
            next = next
                .checked_add(1)
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
            let id = EntityId(next);
            if !self.locations.contains_key(&id) {
                return Ok(id);
            }
        }
    }

    fn vehicle_component_ids(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<HashSet<EntityId>, RegionOwnerLaneError> {
        let mut component = HashSet::new();
        for &entity in entities {
            let mut leader = entity;
            let mut steps = 0usize;
            while let Some(&vehicle) = self.passenger_vehicles.get(&leader) {
                steps += 1;
                if steps > self.locations.len() {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                leader = vehicle;
            }
            steps = 0;
            loop {
                steps += 1;
                if steps > self.locations.len().saturating_add(1) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                component.insert(leader);
                let Some(&passenger) = self.vehicle_passengers.get(&leader) else {
                    break;
                };
                leader = passenger;
            }
        }
        Ok(component)
    }

    fn index_vehicle_link(&mut self, snapshot: &EntitySnapshot) {
        let Some(passenger) = snapshot_vehicle_reference(snapshot) else {
            return;
        };
        debug_assert!(
            self.vehicle_passengers
                .insert(snapshot.id, passenger)
                .is_none()
        );
        debug_assert!(
            self.passenger_vehicles
                .insert(passenger, snapshot.id)
                .is_none()
        );
    }

    fn remove_vehicle_links(&mut self, snapshot: &EntitySnapshot) {
        if let Some(passenger) = self.vehicle_passengers.remove(&snapshot.id)
            && self.passenger_vehicles.get(&passenger).copied() == Some(snapshot.id)
        {
            self.passenger_vehicles.remove(&passenger);
        }
        if let Some(vehicle) = self.passenger_vehicles.remove(&snapshot.id)
            && self.vehicle_passengers.get(&vehicle).copied() == Some(snapshot.id)
        {
            self.vehicle_passengers.remove(&vehicle);
        }
    }

    pub fn set_animal_states_if_current(
        &mut self,
        states: impl IntoIterator<Item = (EntitySnapshot, AnimalBreedingState)>,
    ) -> Result<bool, RegionOwnerLaneError> {
        self.set_animal_states_if_current_inner(states, true)
    }

    fn set_animal_states_if_current_inner(
        &mut self,
        states: impl IntoIterator<Item = (EntitySnapshot, AnimalBreedingState)>,
        journal_commit: bool,
    ) -> Result<bool, RegionOwnerLaneError> {
        let states = states.into_iter().collect::<Vec<_>>();
        if states.is_empty() {
            return Ok(true);
        }
        let mut ids = HashSet::with_capacity(states.len());
        for (expected, _) in &states {
            if expected.animal.is_none() || !ids.insert(expected.id) {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            if self.locations.get(&expected.id).copied()
                != RegionKey::from_position(expected.position)
            {
                return Ok(false);
            }
        }
        let (first_sequence, next_sequence) = self.commit_state.reserve_sequences(states.len())?;
        let mut mutations = BTreeMap::new();
        for (offset, (expected, animal)) in states.into_iter().enumerate() {
            let key = self.locations[&expected.id];
            let lease = self
                .ownership
                .lease(key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            mutations
                .entry(lease.lane)
                .or_insert_with(Vec::new)
                .push(SequencedRegionMutation {
                    sequence: first_sequence + offset as u64 + 1,
                    lease,
                    mutation: RegionOwnerMutation::SetAnimalStateIfCurrent {
                        expected: Box::new(expected),
                        animal,
                    },
                });
        }
        match self.execute_mutations_with_stats(mutations, next_sequence, journal_commit) {
            Ok(_) => Ok(true),
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn set_goal(
        &mut self,
        entity: EntityId,
        goal: GoalState,
    ) -> Result<bool, RegionOwnerLaneError> {
        if goal_reference(&goal).is_some_and(|target| !self.locations.contains_key(&target)) {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        let Some(expected) = self.snapshot(entity)? else {
            return Ok(false);
        };
        let key = self.locations[&entity];
        let lease = self
            .ownership
            .lease(key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let (_, sequence) = self.commit_state.reserve_sequences(1)?;
        let mutations = BTreeMap::from([(
            lease.lane,
            vec![SequencedRegionMutation {
                sequence,
                lease,
                mutation: RegionOwnerMutation::SetGoalIfCurrent {
                    expected: Box::new(expected),
                    goal,
                },
            }],
        )]);
        match self.execute_mutations(mutations, sequence) {
            Ok(()) => Ok(true),
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn set_goals(
        &mut self,
        goals: impl IntoIterator<Item = (EntityId, GoalState)>,
    ) -> Result<usize, RegionOwnerLaneError> {
        self.set_goals_inner(goals, true)
    }

    fn set_goals_inner(
        &mut self,
        goals: impl IntoIterator<Item = (EntityId, GoalState)>,
        journal_commit: bool,
    ) -> Result<usize, RegionOwnerLaneError> {
        let goals = goals.into_iter().collect::<Vec<_>>();
        if goals.is_empty() {
            return Ok(0);
        }
        let mut ids = HashSet::with_capacity(goals.len());
        for (entity, goal) in &goals {
            if !ids.insert(*entity)
                || goal_reference(goal).is_some_and(|target| !self.locations.contains_key(&target))
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
        }
        let current = self
            .snapshots()?
            .into_iter()
            .map(|snapshot| (snapshot.id, snapshot))
            .collect::<HashMap<_, _>>();
        if goals
            .iter()
            .any(|(entity, _)| !current.contains_key(entity))
        {
            return Ok(0);
        }
        let (first_sequence, next_sequence) = self.commit_state.reserve_sequences(goals.len())?;
        let mut mutations = BTreeMap::new();
        for (offset, (entity, goal)) in goals.into_iter().enumerate() {
            let key = self.locations[&entity];
            let lease = self
                .ownership
                .lease(key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            mutations
                .entry(lease.lane)
                .or_insert_with(Vec::new)
                .push(SequencedRegionMutation {
                    sequence: first_sequence + offset as u64 + 1,
                    lease,
                    mutation: RegionOwnerMutation::SetGoalIfCurrent {
                        expected: Box::new(current[&entity].clone()),
                        goal,
                    },
                });
        }
        match self.execute_mutations_with_stats(mutations, next_sequence, journal_commit) {
            Ok(_) => Ok(ids.len()),
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(0),
            Err(error) => Err(error),
        }
    }

    pub fn set_item_stack(
        &mut self,
        entity: EntityId,
        item_stack: Option<EntityItemStack>,
    ) -> Result<bool, RegionOwnerLaneError> {
        let Some(expected) = self.snapshot(entity)? else {
            return Ok(false);
        };
        self.set_item_stack_if_current(expected, item_stack)
    }

    pub fn set_item_stack_if_current(
        &mut self,
        expected: EntitySnapshot,
        item_stack: Option<EntityItemStack>,
    ) -> Result<bool, RegionOwnerLaneError> {
        let Some(key) = RegionKey::from_position(expected.position) else {
            return Err(RegionOwnerLaneError::InvalidMutation);
        };
        if self.locations.get(&expected.id).copied() != Some(key) {
            return Ok(false);
        }
        let lease = self
            .ownership
            .lease(key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let (_, sequence) = self.commit_state.reserve_sequences(1)?;
        let mutations = BTreeMap::from([(
            lease.lane,
            vec![SequencedRegionMutation {
                sequence,
                lease,
                mutation: RegionOwnerMutation::SetItemStackIfCurrent {
                    expected: Box::new(expected),
                    item_stack,
                },
            }],
        )]);
        match self.execute_mutations(mutations, sequence) {
            Ok(()) => Ok(true),
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn set_position(
        &mut self,
        entity: EntityId,
        position: Vec3,
    ) -> Result<bool, RegionOwnerLaneError> {
        if !position.is_finite() {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        let Some(expected) = self.snapshot(entity)? else {
            return Ok(false);
        };
        let state = EntityKinematics {
            id: entity,
            position,
            rotation: expected.rotation,
            velocity: expected.velocity,
            on_ground: expected.on_ground,
        };
        self.apply_kinematics_if_current([(expected, state)])
    }

    pub fn damage(
        &mut self,
        entity: EntityId,
        amount: f32,
    ) -> Result<Option<EntityDamage>, RegionOwnerLaneError> {
        let Some(expected) = self.snapshot(entity)? else {
            return Ok(None);
        };
        self.damage_if_current(expected, amount)
    }

    pub fn apply_kinematics_if_current(
        &mut self,
        states: impl IntoIterator<Item = (EntitySnapshot, EntityKinematics)>,
    ) -> Result<bool, RegionOwnerLaneError> {
        self.apply_kinematics_if_current_inner(states, true)
    }

    fn apply_kinematics_if_current_inner(
        &mut self,
        states: impl IntoIterator<Item = (EntitySnapshot, EntityKinematics)>,
        journal_commit: bool,
    ) -> Result<bool, RegionOwnerLaneError> {
        let mut states = states.into_iter().collect::<Vec<_>>();
        if states.is_empty() {
            return Ok(true);
        }
        states.sort_unstable_by_key(|(expected, _)| expected.id);
        let mut ids = HashSet::with_capacity(states.len());
        for (expected, state) in &states {
            if expected.id != state.id
                || RegionKey::from_position(expected.position).is_none()
                || !state.is_finite()
                || !ids.insert(expected.id)
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
        }
        let standalone_local = states.iter().all(|(expected, state)| {
            let source = RegionKey::from_position(expected.position);
            self.locations.get(&expected.id).copied() == source
                && RegionKey::from_position(state.position) == source
                && !self.vehicle_passengers.contains_key(&expected.id)
                && !self.passenger_vehicles.contains_key(&expected.id)
                && snapshot_vehicle_reference(expected).is_none()
        });
        if standalone_local {
            let mut states_by_region =
                BTreeMap::<RegionKey, Vec<(EntitySnapshot, EntityKinematics)>>::new();
            for (expected, state) in states {
                let source = RegionKey::from_position(expected.position)
                    .ok_or(RegionOwnerLaneError::InvalidMutation)?;
                states_by_region
                    .entry(source)
                    .or_default()
                    .push((expected, state));
            }
            let (first_sequence, next_sequence) = self
                .commit_state
                .reserve_sequences(states_by_region.len())?;
            let mut mutations = BTreeMap::<usize, Vec<SequencedRegionMutation>>::new();
            for (offset, (source, states)) in states_by_region.into_iter().enumerate() {
                let lease = self
                    .ownership
                    .lease(source)
                    .ok_or(RegionOwnerLaneError::StaleLease)?;
                let (expected, states) = states.into_iter().unzip();
                mutations
                    .entry(lease.lane)
                    .or_default()
                    .push(SequencedRegionMutation {
                        sequence: first_sequence + offset as u64 + 1,
                        lease,
                        mutation: RegionOwnerMutation::SetKinematicsBatchIfCurrent {
                            expected,
                            states,
                        },
                    });
            }
            return match self.execute_mutations_with_stats(mutations, next_sequence, journal_commit)
            {
                Ok(_) => Ok(true),
                Err(RegionOwnerLaneError::InvalidMutation) => Ok(false),
                Err(error) => Err(error),
            };
        }
        let component_ids = self.vehicle_component_ids(&ids)?;
        let snapshots = if component_ids.len() == self.locations.len() {
            self.snapshots()?
        } else {
            self.snapshots_for_ids(&component_ids)?
        };
        let current = snapshots
            .into_iter()
            .map(|snapshot| (snapshot.id, snapshot))
            .collect::<BTreeMap<_, _>>();
        if current.len() != component_ids.len()
            || current.values().any(|snapshot| {
                snapshot_vehicle_reference(snapshot)
                    != self.vehicle_passengers.get(&snapshot.id).copied()
                    || self
                        .passenger_vehicles
                        .get(&snapshot.id)
                        .is_some_and(|vehicle| {
                            self.vehicle_passengers.get(vehicle).copied() != Some(snapshot.id)
                        })
            })
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        if states
            .iter()
            .any(|(expected, _)| current.get(&expected.id) != Some(expected))
        {
            return Ok(false);
        }
        let input = states
            .iter()
            .map(|(expected, state)| (expected.id, (expected, state)))
            .collect::<HashMap<_, _>>();
        let parents = current
            .values()
            .filter_map(|snapshot| {
                snapshot_vehicle_reference(snapshot).map(|passenger| (passenger, snapshot.id))
            })
            .collect::<HashMap<_, _>>();
        let mut handled = HashSet::new();
        let mut plans = Vec::new();
        let mut target_regions = BTreeSet::new();
        for (expected, state) in &states {
            if handled.contains(&expected.id) {
                continue;
            }
            let source = self.locations[&expected.id];
            let mut leader = expected.id;
            while let Some(parent) = parents.get(&leader).copied() {
                leader = parent;
            }
            let mut group = Vec::new();
            let mut member = Some(leader);
            while let Some(entity) = member {
                let snapshot = current
                    .get(&entity)
                    .ok_or(RegionOwnerLaneError::InvalidMutation)?;
                if self.locations.get(&entity).copied() != Some(source) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                group.push(snapshot.clone());
                member = snapshot_vehicle_reference(snapshot);
            }
            let leader_state = input.get(&leader).map(|(_, state)| **state);
            let target = leader_state
                .and_then(|state| RegionKey::from_position(state.position))
                .unwrap_or(source);
            if group.len() > 1 && target != source {
                let leader_state = leader_state.ok_or(RegionOwnerLaneError::InvalidMutation)?;
                let leader_snapshot = current
                    .get(&leader)
                    .ok_or(RegionOwnerLaneError::InvalidMutation)?;
                let delta = Vec3::new(
                    leader_state.position.x - leader_snapshot.position.x,
                    leader_state.position.y - leader_snapshot.position.y,
                    leader_state.position.z - leader_snapshot.position.z,
                );
                let mut moved = Vec::with_capacity(group.len());
                for snapshot in &group {
                    let mut target_snapshot = snapshot.clone();
                    if snapshot.id == leader {
                        target_snapshot.position = leader_state.position;
                        target_snapshot.rotation = leader_state.rotation;
                        target_snapshot.velocity = leader_state.velocity;
                        target_snapshot.on_ground = leader_state.on_ground;
                    } else {
                        target_snapshot.position = Vec3::new(
                            snapshot.position.x + delta.x,
                            snapshot.position.y + delta.y,
                            snapshot.position.z + delta.z,
                        );
                    }
                    if RegionKey::from_position(target_snapshot.position) != Some(target) {
                        return Ok(false);
                    }
                    moved.push(target_snapshot);
                }
                target_regions.insert(target);
                handled.extend(group.iter().map(|snapshot| snapshot.id));
                plans.push(OwnerKinematicsPlan::Migrate {
                    source,
                    target,
                    expected: group,
                    moved,
                });
                continue;
            }

            let target = RegionKey::from_position(state.position)
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
            if group.len() > 1 && target != source {
                return Ok(false);
            }
            handled.insert(expected.id);
            if source == target {
                plans.push(OwnerKinematicsPlan::Local {
                    source,
                    expected: Box::new(expected.clone()),
                    state: *state,
                });
            } else {
                let mut moved = expected.clone();
                moved.position = state.position;
                moved.rotation = state.rotation;
                moved.velocity = state.velocity;
                moved.on_ground = state.on_ground;
                target_regions.insert(target);
                plans.push(OwnerKinematicsPlan::Migrate {
                    source,
                    target,
                    expected: vec![expected.clone()],
                    moved: vec![moved],
                });
            }
        }
        for target in target_regions {
            self.ensure_region(target)?;
        }
        let mutation_count = plans
            .iter()
            .map(|plan| match plan {
                OwnerKinematicsPlan::Local { .. } => 1usize,
                OwnerKinematicsPlan::Migrate { .. } => 2usize,
            })
            .try_fold(0usize, usize::checked_add)
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let (first_sequence, next_sequence) =
            self.commit_state.reserve_sequences(mutation_count)?;
        let mut mutations = BTreeMap::new();
        let mut migrations = Vec::new();
        let mut sequence = first_sequence;
        for plan in plans {
            match plan {
                OwnerKinematicsPlan::Local {
                    source,
                    expected,
                    state,
                } => {
                    let lease = self
                        .ownership
                        .lease(source)
                        .ok_or(RegionOwnerLaneError::StaleLease)?;
                    sequence += 1;
                    mutations.entry(lease.lane).or_insert_with(Vec::new).push(
                        SequencedRegionMutation {
                            sequence,
                            lease,
                            mutation: RegionOwnerMutation::SetKinematicsIfCurrent {
                                expected,
                                state,
                            },
                        },
                    );
                }
                OwnerKinematicsPlan::Migrate {
                    source,
                    target,
                    expected,
                    moved,
                } => {
                    let source_lease = self
                        .ownership
                        .lease(source)
                        .ok_or(RegionOwnerLaneError::StaleLease)?;
                    sequence += 1;
                    mutations
                        .entry(source_lease.lane)
                        .or_insert_with(Vec::new)
                        .push(SequencedRegionMutation {
                            sequence,
                            lease: source_lease,
                            mutation: RegionOwnerMutation::RemoveSnapshotsIfCurrent(expected),
                        });
                    let target_lease = self
                        .ownership
                        .lease(target)
                        .ok_or(RegionOwnerLaneError::StaleLease)?;
                    sequence += 1;
                    migrations.extend(moved.iter().map(|snapshot| (snapshot.id, target)));
                    mutations
                        .entry(target_lease.lane)
                        .or_insert_with(Vec::new)
                        .push(SequencedRegionMutation {
                            sequence,
                            lease: target_lease,
                            mutation: RegionOwnerMutation::InsertSnapshots(moved),
                        });
                }
            }
        }
        debug_assert_eq!(sequence, next_sequence);
        match self.execute_mutations_with_stats(mutations, next_sequence, journal_commit) {
            Ok(_) => {
                for (entity, target) in migrations {
                    self.locations.insert(entity, target);
                }
                Ok(true)
            }
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn damage_if_current(
        &mut self,
        expected: EntitySnapshot,
        amount: f32,
    ) -> Result<Option<EntityDamage>, RegionOwnerLaneError> {
        if !amount.is_finite() {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        let Some(key) = RegionKey::from_position(expected.position) else {
            return Err(RegionOwnerLaneError::InvalidMutation);
        };
        if self.locations.get(&expected.id).copied() != Some(key) {
            return Ok(None);
        }
        let lease = self
            .ownership
            .lease(key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let (_, sequence) = self.commit_state.reserve_sequences(1)?;
        let entity = expected.id;
        let mutations = BTreeMap::from([(
            lease.lane,
            vec![SequencedRegionMutation {
                sequence,
                lease,
                mutation: RegionOwnerMutation::DamageIfCurrent {
                    expected: Box::new(expected),
                    amount,
                },
            }],
        )]);
        match self.execute_mutations(mutations, sequence) {
            Ok(()) => {
                let snapshot = self
                    .snapshot(entity)?
                    .ok_or(RegionOwnerLaneError::UnknownEntity)?;
                Ok(Some(EntityDamage {
                    killed: snapshot.lifecycle == crate::EntityLifecycle::Despawning,
                    snapshot,
                }))
            }
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn set_velocities(
        &mut self,
        velocities: impl IntoIterator<Item = (EntityId, Vec3)>,
    ) -> Result<(), RegionOwnerLaneError> {
        let velocities = velocities.into_iter().collect::<Vec<_>>();
        if velocities.is_empty() {
            return Ok(());
        }
        let mut ids = HashSet::with_capacity(velocities.len());
        for (entity, velocity) in &velocities {
            if !ids.insert(*entity) || !velocity.is_finite() {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            if !self.locations.contains_key(entity) {
                return Err(RegionOwnerLaneError::UnknownEntity);
            }
        }
        let (first_sequence, next_sequence) =
            self.commit_state.reserve_sequences(velocities.len())?;
        let mut mutations = BTreeMap::<usize, Vec<SequencedRegionMutation>>::new();
        for (offset, (entity, velocity)) in velocities.into_iter().enumerate() {
            let key = self.locations[&entity];
            let lease = self.ownership.lease(key).expect("indexed entity lease");
            mutations
                .entry(lease.lane)
                .or_default()
                .push(SequencedRegionMutation {
                    sequence: first_sequence + offset as u64 + 1,
                    lease,
                    mutation: RegionOwnerMutation::SetVelocity { entity, velocity },
                });
        }
        self.execute_mutations(mutations, next_sequence)
    }

    pub fn prepare_goal_tick_with_pathing_for_ids(
        &self,
        tick: u64,
        active_ids: &HashSet<EntityId>,
    ) -> Result<RegionalPreparedGoalTick, RegionOwnerLaneError> {
        self.prepare_goal_tick_with_pathing_for_ids_from_snapshots(tick, active_ids, None)
    }

    fn prepare_goal_tick_with_pathing_for_ids_from_snapshots(
        &self,
        tick: u64,
        active_ids: &HashSet<EntityId>,
        selected: Option<&[EntitySnapshot]>,
    ) -> Result<RegionalPreparedGoalTick, RegionOwnerLaneError> {
        let mut snapshots = BTreeMap::new();
        let mut selected_is_complete = selected.is_some();
        if let Some(selected) = selected {
            for snapshot in selected {
                if active_ids.contains(&snapshot.id)
                    && snapshots.insert(snapshot.id, snapshot.clone()).is_some()
                {
                    selected_is_complete = false;
                    break;
                }
            }
            selected_is_complete &= snapshots.len() == active_ids.len();
        }
        if !selected_is_complete {
            snapshots = self
                .snapshots_for_ids(active_ids)?
                .into_iter()
                .map(|snapshot| (snapshot.id, snapshot))
                .collect();
        }
        let referenced_target_ids = snapshots
            .values()
            .filter_map(|snapshot| goal_reference(&snapshot.goal))
            .collect::<HashSet<_>>();
        let target_ids = referenced_target_ids
            .iter()
            .copied()
            .filter(|target| !snapshots.contains_key(target))
            .filter(|target| self.locations.contains_key(target))
            .collect::<HashSet<_>>();
        snapshots.extend(
            self.snapshots_for_ids(&target_ids)?
                .into_iter()
                .map(|snapshot| (snapshot.id, snapshot)),
        );
        let expected_missing_follow_targets = referenced_target_ids
            .into_iter()
            .filter(|target| !snapshots.contains_key(target))
            .collect::<HashSet<_>>();
        let mut ids_by_region = BTreeMap::<RegionKey, HashSet<EntityId>>::new();
        for id in active_ids {
            if let Some(key) = self.locations.get(id).copied() {
                ids_by_region.entry(key).or_default().insert(*id);
            }
        }

        let mut follow_targets = BTreeMap::<RegionKey, HashMap<EntityId, Vec3>>::new();
        let mut follow_target_sources = BTreeMap::new();
        let mut goal_inputs = BTreeMap::new();
        for (&region, ids) in &ids_by_region {
            for id in ids {
                let follower = snapshots
                    .get(id)
                    .ok_or(RegionOwnerLaneError::UnknownEntity)?;
                goal_inputs.insert(*id, follower.clone());
                let Some(target) = goal_reference(&follower.goal) else {
                    continue;
                };
                let Some(target_region) = self.locations.get(&target).copied() else {
                    continue;
                };
                let target_snapshot = snapshots
                    .get(&target)
                    .ok_or(RegionOwnerLaneError::UnknownEntity)?;
                follow_target_sources.insert(
                    target,
                    RegionalFollowTargetSource {
                        region: target_region,
                        snapshot: target_snapshot.clone(),
                    },
                );
                if target_region != region {
                    follow_targets
                        .entry(region)
                        .or_default()
                        .insert(target, target_snapshot.position);
                }
            }
        }

        let mut pending = Vec::with_capacity(ids_by_region.len());
        for (key, ids) in ids_by_region {
            let lease = self
                .ownership
                .lease(key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            let completion = self
                .lanes
                .get(&lease.lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?
                .request_goal_tick(lease, tick, ids)?;
            pending.push((key, completion));
        }
        let mut batches = BTreeMap::new();
        for (key, completion) in pending {
            let prepared = completion
                .recv()
                .map_err(|_| RegionOwnerLaneError::Closed)??;
            batches.insert(key, prepared);
        }
        let lease_keys = batches
            .keys()
            .copied()
            .chain(follow_target_sources.values().map(|source| source.region))
            .collect::<BTreeSet<_>>();
        Ok(RegionalPreparedGoalTick {
            authority: self.authority,
            phase: RegionPhase(0),
            leases: lease_keys
                .into_iter()
                .map(|key| {
                    self.ownership
                        .lease(key)
                        .map(|lease| (key, lease))
                        .ok_or(RegionOwnerLaneError::StaleLease)
                })
                .collect::<Result<_, _>>()?,
            batches,
            follow_targets,
            follow_target_sources,
            expected_missing_follow_targets,
            goal_inputs,
        })
    }

    pub fn apply_prepared_goal_tick(
        &mut self,
        resolved: RegionalResolvedGoalTick,
    ) -> Result<GoalTickStats, RegionOwnerLaneError> {
        self.apply_prepared_goal_tick_inner(resolved, true)
    }

    fn apply_prepared_goal_tick_inner(
        &mut self,
        resolved: RegionalResolvedGoalTick,
        journal_commit: bool,
    ) -> Result<GoalTickStats, RegionOwnerLaneError> {
        Ok(self
            .try_apply_prepared_goal_tick_inner(resolved, journal_commit)?
            .unwrap_or_default())
    }

    fn apply_prepared_goal_tick_and_kinematics_for_ids(
        &mut self,
        resolved: RegionalResolvedGoalTick,
        entities: &HashSet<EntityId>,
        journal_commit: bool,
    ) -> Result<Option<(GoalTickStats, Vec<EntityKinematics>)>, RegionOwnerLaneError> {
        let Some(stats) = self.try_apply_prepared_goal_tick_inner(resolved, journal_commit)? else {
            return Ok(None);
        };
        let states = self.alive_kinematics_for_ids(entities)?;
        Ok(Some((stats, states)))
    }

    fn try_apply_prepared_goal_tick_inner(
        &mut self,
        mut resolved: RegionalResolvedGoalTick,
        journal_commit: bool,
    ) -> Result<Option<GoalTickStats>, RegionOwnerLaneError> {
        if resolved.authority != self.authority || resolved.phase != RegionPhase(0) {
            return Ok(None);
        }
        if resolved
            .leases
            .iter()
            .any(|(key, lease)| lease.key != *key || !self.ownership.validate(*lease))
        {
            return Ok(None);
        }
        if resolved
            .expected_missing_follow_targets
            .iter()
            .any(|target| self.locations.contains_key(target))
        {
            return Ok(None);
        }
        if resolved.goal_inputs.values().any(|expected| {
            self.locations.get(&expected.id).copied() != RegionKey::from_position(expected.position)
        }) {
            return Ok(None);
        }

        let validation_ids = resolved
            .follow_target_sources
            .keys()
            .copied()
            .collect::<HashSet<_>>();
        let current = self
            .snapshots_for_ids(&validation_ids)?
            .into_iter()
            .map(|snapshot| (snapshot.id, snapshot))
            .collect::<BTreeMap<_, _>>();
        if resolved.follow_target_sources.iter().any(|(id, expected)| {
            self.locations.get(id).copied() != Some(expected.region)
                || current.get(id) != Some(&expected.snapshot)
        }) {
            return Ok(None);
        }
        if resolved.batches.is_empty() {
            return Ok(None);
        }

        let batch_count = resolved.batches.len();
        let (first_sequence, next_sequence) = self.commit_state.reserve_sequences(batch_count)?;
        let mut mutations = BTreeMap::<usize, Vec<SequencedRegionMutation>>::new();
        for (offset, (key, batch)) in resolved.batches.into_iter().enumerate() {
            let lease = self
                .ownership
                .lease(key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            let expected = resolved
                .goal_inputs
                .values()
                .filter(|snapshot| self.locations.get(&snapshot.id).copied() == Some(key))
                .cloned()
                .collect();
            mutations
                .entry(lease.lane)
                .or_default()
                .push(SequencedRegionMutation {
                    sequence: first_sequence + offset as u64 + 1,
                    lease,
                    mutation: RegionOwnerMutation::ApplyGoalBatch {
                        expected,
                        resolved: Box::new(batch),
                        follow_targets: resolved.follow_targets.remove(&key).unwrap_or_default(),
                    },
                });
        }
        match self.execute_mutations_with_stats(mutations, next_sequence, journal_commit) {
            Ok(stats) => Ok(Some(stats)),
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn execute_mutations(
        &mut self,
        mutations: BTreeMap<usize, Vec<SequencedRegionMutation>>,
        sequence_watermark: u64,
    ) -> Result<(), RegionOwnerLaneError> {
        self.execute_mutations_with_stats(mutations, sequence_watermark, true)
            .map(|_| ())
    }

    fn execute_mutations_with_stats(
        &mut self,
        mut mutations: BTreeMap<usize, Vec<SequencedRegionMutation>>,
        sequence_watermark: u64,
        journal_commit: bool,
    ) -> Result<GoalTickStats, RegionOwnerLaneError> {
        let journal_enabled = journal_commit
            && self
                .commit_state
                .journal
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .enabled();
        let journal_entities = journal_enabled.then(|| journal_entities_by_lane(&mutations));
        let participants = mutations.keys().copied().collect::<BTreeSet<_>>();
        let phase = self.commit_state.reserve_phase()?;
        self.ownership
            .begin_allocated_phase_for_lanes(phase, participants.clone())
            .map_err(|_| RegionOwnerLaneError::Busy)?;
        let mut goal_stats = GoalTickStats::default();

        let mut prepared = Vec::with_capacity(participants.len());
        let mut prepared_and_committed = Vec::new();
        let mut first_error = None;
        for (index, &lane) in participants.iter().enumerate() {
            let owner = self
                .lanes
                .get(&lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?;
            let batch = RegionOwnerBatch {
                phase,
                sequence_watermark,
                mutations: mutations.remove(&lane).unwrap_or_default(),
            };
            if participants.len() == 1 && index == 0 {
                match owner.prepare_and_commit(batch) {
                    Ok(completion) => prepared_and_committed.push((lane, completion)),
                    Err(error) => first_error = first_error.or(Some(error)),
                }
            } else {
                match owner.prepare(batch) {
                    Ok(completion) => prepared.push((lane, completion)),
                    Err(error) => first_error = first_error.or(Some(error)),
                }
            }
        }
        let mut ready = BTreeSet::new();
        let mut settled = BTreeSet::new();
        let mut applied = BTreeSet::new();
        for (lane, completion) in prepared {
            match completion.recv() {
                Ok(Ok(prepared_phase)) if prepared_phase == phase => {
                    ready.insert(lane);
                }
                Ok(Err(error)) => {
                    first_error = first_error.or(Some(error));
                    settled.insert(lane);
                }
                Ok(Ok(_)) => {
                    first_error = first_error.or(Some(RegionOwnerLaneError::StalePhase));
                    settled.insert(lane);
                }
                Err(_) => first_error = first_error.or(Some(RegionOwnerLaneError::Closed)),
            }
        }
        for (lane, completion) in prepared_and_committed {
            match completion.recv() {
                Ok(Ok(result)) if result.phase == phase => {
                    add_goal_tick_stats(&mut goal_stats, result.goal_stats);
                    ready.insert(lane);
                    applied.insert(lane);
                }
                Ok(Err(error)) => {
                    first_error = first_error.or(Some(error));
                    settled.insert(lane);
                }
                Ok(Ok(_)) => {
                    first_error = first_error.or(Some(RegionOwnerLaneError::StalePhase));
                    settled.insert(lane);
                }
                Err(_) => first_error = first_error.or(Some(RegionOwnerLaneError::Closed)),
            }
        }

        if first_error.is_none() && ready.len() == participants.len() {
            let mut committed = Vec::with_capacity(ready.len() - applied.len());
            for &lane in ready.difference(&applied) {
                match self.lanes[&lane].commit(phase) {
                    Ok(completion) => committed.push((lane, completion)),
                    Err(error) => first_error = first_error.or(Some(error)),
                }
            }
            for (lane, completion) in committed {
                match completion.recv() {
                    Ok(Ok(result)) if result.phase == phase => {
                        add_goal_tick_stats(&mut goal_stats, result.goal_stats);
                        applied.insert(lane);
                    }
                    Ok(Err(error)) => {
                        first_error = first_error.or(Some(error));
                        settled.insert(lane);
                    }
                    Ok(Ok(_)) => {
                        first_error = first_error.or(Some(RegionOwnerLaneError::StalePhase));
                    }
                    Err(_) => first_error = first_error.or(Some(RegionOwnerLaneError::Closed)),
                }
            }
            if first_error.is_none()
                && applied.len() == participants.len()
                && let Some(entities) = journal_entities.as_ref()
                && let Err(error) = self.record_commit_decision(phase, sequence_watermark, entities)
            {
                if error.outcome_unknown() {
                    self.commit_state
                        .journal_phases
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .insert(phase, sequence_watermark);
                    self.commit_state
                        .outcome_unknown
                        .store(true, Ordering::Release);
                    return Err(RegionOwnerLaneError::Journal);
                }
                first_error = Some(RegionOwnerLaneError::Journal);
            }
            if first_error.is_none() && applied.len() == participants.len() {
                for lane in applied {
                    match self.lanes[&lane].finalize(phase) {
                        Ok(completion) => match completion.recv() {
                            Ok(Ok(finalized)) if finalized == phase => {
                                settled.insert(lane);
                            }
                            Ok(Err(error)) => first_error = first_error.or(Some(error)),
                            _ => {
                                first_error = first_error.or(Some(RegionOwnerLaneError::Closed));
                            }
                        },
                        Err(error) => first_error = first_error.or(Some(error)),
                    }
                }
            } else {
                for lane in applied {
                    match self.lanes[&lane].rollback(phase) {
                        Ok(completion) => match completion.recv() {
                            Ok(Ok(rolled_back)) if rolled_back == phase => {
                                settled.insert(lane);
                            }
                            Ok(Err(error)) => first_error = first_error.or(Some(error)),
                            _ => {
                                first_error = first_error.or(Some(RegionOwnerLaneError::Closed));
                            }
                        },
                        Err(error) => first_error = first_error.or(Some(error)),
                    }
                }
                for lane in ready.difference(&settled).copied().collect::<Vec<_>>() {
                    match self.lanes[&lane].abort(phase) {
                        Ok(completion) => {
                            if matches!(completion.recv(), Ok(Ok(aborted)) if aborted == phase) {
                                settled.insert(lane);
                            }
                        }
                        Err(error) => first_error = first_error.or(Some(error)),
                    }
                }
            }
        } else {
            let mut aborted = Vec::with_capacity(ready.len());
            for lane in ready {
                match self.lanes[&lane].abort(phase) {
                    Ok(completion) => aborted.push((lane, completion)),
                    Err(error) => first_error = first_error.or(Some(error)),
                }
            }
            for (lane, completion) in aborted {
                if matches!(completion.recv(), Ok(Ok(aborted_phase)) if aborted_phase == phase) {
                    settled.insert(lane);
                } else {
                    first_error = first_error.or(Some(RegionOwnerLaneError::Closed));
                }
            }
        }
        for lane in settled {
            if self.ownership.acknowledge_lane(phase, lane).is_err() {
                first_error = first_error.or(Some(RegionOwnerLaneError::StalePhase));
            }
        }
        if self.ownership.finish_phase(phase).is_err() {
            return Err(first_error.unwrap_or(RegionOwnerLaneError::StalePhase));
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(goal_stats),
        }
    }

    fn record_commit_decision(
        &mut self,
        phase: RegionPhase,
        sequence_watermark: u64,
        entities_by_lane: &BTreeMap<usize, Vec<(RegionLease, EntityId)>>,
    ) -> Result<(), RegionalDecisionJournalError> {
        let mut pending = Vec::with_capacity(entities_by_lane.len());
        for (&lane, entities) in entities_by_lane {
            let snapshots = self
                .lanes
                .get(&lane)
                .ok_or(RegionalDecisionJournalError::SAFE)?
                .request_existing_snapshots_for_ids(entities.clone())
                .map_err(|_| RegionalDecisionJournalError::SAFE)?;
            pending.push(snapshots);
        }
        let affected = entities_by_lane
            .values()
            .flatten()
            .map(|(_, entity)| *entity)
            .collect::<BTreeSet<_>>();
        let mut upserts = Vec::with_capacity(affected.len());
        for snapshots in pending {
            upserts.extend(
                snapshots
                    .recv()
                    .map_err(|_| RegionalDecisionJournalError::SAFE)?
                    .map_err(|_| RegionalDecisionJournalError::SAFE)?,
            );
        }
        upserts.sort_unstable_by_key(|snapshot| snapshot.id);
        upserts.dedup_by_key(|snapshot| snapshot.id);
        let present = upserts
            .iter()
            .map(|snapshot| snapshot.id)
            .collect::<BTreeSet<_>>();
        let removed = affected.difference(&present).copied().collect();
        self.commit_state.record_commit(&RegionalCommitDecision {
            phase,
            sequence_watermark,
            upserts,
            removed,
        })
    }

    pub fn shutdown(self) -> Result<RegionalEntityStore, RegionalOwnerShutdownError> {
        let Self {
            authority,
            mut ownership,
            mut locations,
            mut uuids,
            vehicle_passengers: _,
            passenger_vehicles: _,
            transfers,
            in_flight_transfers,
            #[cfg(any(test, feature = "shadow-compare"))]
            shadow_stats,
            next_id,
            lanes,
            commit_state,
        } = self;
        ownership.last_phase = ownership
            .last_phase
            .max(commit_state.next_phase.load(Ordering::Acquire));
        let mut stores = BTreeMap::new();
        let mut first_error = None;
        for (_, lane) in lanes {
            match lane.shutdown() {
                Ok(owned) => {
                    for (key, store) in owned {
                        if stores.insert(key, store).is_some() {
                            first_error =
                                first_error.or(Some(RegionOwnerLaneError::DuplicateRegion));
                        }
                    }
                }
                Err(error) => first_error = first_error.or(Some(error)),
            }
        }
        if first_error.is_some() {
            retain_recovered_regional_state(&mut ownership, &stores, &mut locations, &mut uuids);
        }
        let recovered = RegionalEntityStore {
            authority,
            ownership,
            stores,
            locations,
            uuids,
            transfers,
            in_flight_transfers,
            #[cfg(any(test, feature = "shadow-compare"))]
            shadow_stats,
            next_id,
        };
        match first_error {
            Some(error) => Err(RegionalOwnerShutdownError {
                error,
                recovered: Box::new(recovered),
            }),
            None => Ok(recovered),
        }
    }
}

fn journal_entities_by_lane(
    mutations: &BTreeMap<usize, Vec<SequencedRegionMutation>>,
) -> BTreeMap<usize, Vec<(RegionLease, EntityId)>> {
    let mut entities = BTreeMap::<usize, BTreeMap<EntityId, RegionLease>>::new();
    for (&lane, mutations) in mutations {
        for mutation in mutations {
            for entity in mutation_entity_ids(&mutation.mutation) {
                entities
                    .entry(lane)
                    .or_default()
                    .insert(entity, mutation.lease);
            }
        }
    }
    entities
        .into_iter()
        .map(|(lane, entities)| {
            (
                lane,
                entities
                    .into_iter()
                    .map(|(entity, lease)| (lease, entity))
                    .collect(),
            )
        })
        .collect()
}

fn mutation_entity_ids(mutation: &RegionOwnerMutation) -> Vec<EntityId> {
    match mutation {
        RegionOwnerMutation::SetVelocity { entity, .. }
        | RegionOwnerMutation::SetAnimalState { entity, .. }
        | RegionOwnerMutation::RemoveEntity(entity) => vec![*entity],
        RegionOwnerMutation::SetAnimalStateIfCurrent { expected, .. }
        | RegionOwnerMutation::SetGoalIfCurrent { expected, .. }
        | RegionOwnerMutation::SetItemStackIfCurrent { expected, .. }
        | RegionOwnerMutation::SetKinematicsIfCurrent { expected, .. }
        | RegionOwnerMutation::DamageIfCurrent { expected, .. }
        | RegionOwnerMutation::RemoveIfCurrent(expected)
        | RegionOwnerMutation::InsertSnapshot(expected) => vec![expected.id],
        RegionOwnerMutation::ApplyGoalBatch { expected, .. }
        | RegionOwnerMutation::SetKinematicsBatchIfCurrent { expected, .. }
        | RegionOwnerMutation::InsertSnapshots(expected)
        | RegionOwnerMutation::RemoveSnapshotsIfCurrent(expected) => {
            expected.iter().map(|snapshot| snapshot.id).collect()
        }
    }
}

fn retain_recovered_regional_state(
    ownership: &mut RegionOwnership,
    stores: &BTreeMap<RegionKey, EntityStore>,
    locations: &mut BTreeMap<EntityId, RegionKey>,
    uuids: &mut HashMap<Uuid, EntityId>,
) {
    ownership.owners.retain(|key, _| stores.contains_key(key));
    ownership.active_phase = None;
    ownership.pending_lanes.clear();
    locations.retain(|entity, key| stores.get(key).is_some_and(|store| store.contains(*entity)));
    uuids.retain(|_, entity| locations.contains_key(entity));
}

impl RegionalEntityStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            authority: next_regional_authority_id(),
            ownership: RegionOwnership::new(),
            stores: BTreeMap::new(),
            locations: BTreeMap::new(),
            uuids: HashMap::new(),
            transfers: BTreeMap::new(),
            in_flight_transfers: BTreeMap::new(),
            #[cfg(any(test, feature = "shadow-compare"))]
            shadow_stats: ShadowComparisonStats::default(),
            next_id: 0,
        }
    }

    #[must_use]
    pub fn with_next_id(next_id: i32) -> Self {
        let mut store = Self::new();
        store.next_id = next_id.max(0);
        store
    }

    pub fn assign_region(
        &mut self,
        key: RegionKey,
        lane: usize,
    ) -> Result<RegionLease, RegionEntityStoreError> {
        let lease = self.ownership.assign(key, lane)?;
        self.stores.insert(key, EntityStore::new());
        Ok(lease)
    }

    pub fn reassign_region(
        &mut self,
        expected: RegionLease,
        lane: usize,
    ) -> Result<RegionLease, RegionEntityStoreError> {
        Ok(self.ownership.reassign(expected, lane)?)
    }

    pub fn begin_phase(&mut self) -> Result<RegionPhase, RegionEntityStoreError> {
        Ok(self.ownership.begin_phase()?)
    }

    pub fn finish_phase(&mut self, expected: RegionPhase) -> Result<(), RegionEntityStoreError> {
        self.ownership.validate_finish(expected)?;
        let transfers = self
            .transfers
            .iter()
            .filter_map(|(&id, transfer)| {
                (transfer.phase == expected && transfer.applied.is_none()).then_some(id)
            })
            .collect::<Vec<_>>();
        for transfer in transfers {
            if self
                .transfers
                .get(&transfer)
                .is_some_and(|prepared| prepared.decision.is_none())
            {
                self.decide_transfer(expected, transfer, TransferDecision::Reject)?;
            }
            self.apply_transfer(expected, transfer)?;
        }
        self.ownership.finish_phase(expected)?;
        self.transfers
            .retain(|_, transfer| transfer.phase != expected);
        debug_assert!(self.in_flight_transfers.is_empty());
        Ok(())
    }

    pub fn acknowledge_lane(
        &mut self,
        expected: RegionPhase,
        lane: usize,
    ) -> Result<(), RegionEntityStoreError> {
        Ok(self.ownership.acknowledge_lane(expected, lane)?)
    }

    pub fn prepare_transfer(
        &mut self,
        phase: RegionPhase,
        source: RegionLease,
        target: RegionLease,
        tick: u64,
        state: EntityKinematics,
    ) -> Result<TransferId, RegionEntityStoreError> {
        self.validate_access(phase, source)?;
        self.validate_access(phase, target)?;
        if !state.is_finite() {
            return Err(RegionEntityStoreError::InvalidKinematics);
        }
        if source.key == target.key {
            return Err(RegionEntityStoreError::SameRegionTransfer);
        }
        if RegionKey::from_position(state.position) != Some(target.key) {
            return Err(RegionEntityStoreError::WrongTargetRegion);
        }
        let transfer = TransferId {
            tick,
            source: source.key,
            source_epoch: source.epoch,
            entity: state.id,
        };
        if let Some(existing) = self.transfers.get(&transfer) {
            return if existing.matches_request(phase, source, target, state) {
                Ok(transfer)
            } else {
                Err(RegionEntityStoreError::TransferConflict)
            };
        }
        if self.locations.get(&state.id).copied() != Some(source.key) {
            return Err(RegionEntityStoreError::WrongSourceRegion);
        }

        let source_snapshot = self
            .stores
            .get(&source.key)
            .and_then(|store| store.snapshot(state.id))
            .ok_or(RegionEntityStoreError::UnknownEntity)?;
        let source_snapshots = self.vehicle_group_snapshots(source.key, state.id)?;
        if vehicle_group_leader(&source_snapshots) != Some(state.id) {
            return Err(RegionEntityStoreError::TransferConflict);
        }
        let group_ids = source_snapshots
            .iter()
            .map(|snapshot| snapshot.id)
            .collect::<HashSet<_>>();
        if group_ids
            .iter()
            .any(|id| self.in_flight_transfers.contains_key(id))
        {
            return Err(RegionEntityStoreError::TransferConflict);
        }
        let delta = Vec3::new(
            state.position.x - source_snapshot.position.x,
            state.position.y - source_snapshot.position.y,
            state.position.z - source_snapshot.position.z,
        );
        let mut target_snapshots = Vec::with_capacity(source_snapshots.len());
        for source_snapshot in &source_snapshots {
            let mut target_snapshot = source_snapshot.clone();
            if source_snapshot.id == state.id {
                target_snapshot.position = state.position;
                target_snapshot.rotation = state.rotation;
                target_snapshot.velocity = state.velocity;
                target_snapshot.on_ground = state.on_ground;
            } else {
                target_snapshot.position = Vec3::new(
                    source_snapshot.position.x + delta.x,
                    source_snapshot.position.y + delta.y,
                    source_snapshot.position.z + delta.z,
                );
            }
            if RegionKey::from_position(target_snapshot.position) != Some(target.key) {
                return Err(RegionEntityStoreError::CrossRegionReference);
            }
            if snapshot_vehicle_reference(&target_snapshot).is_some_and(|referenced| {
                !group_ids.contains(&referenced) && self.region_for(referenced) != Some(target.key)
            }) || goal_reference(&target_snapshot.goal)
                .is_some_and(|referenced| !self.locations.contains_key(&referenced))
            {
                return Err(RegionEntityStoreError::CrossRegionReference);
            }
            target_snapshots.push(target_snapshot);
        }
        if self.has_incoming_cross_region_reference(&group_ids, target.key) {
            return Err(RegionEntityStoreError::CrossRegionReference);
        }

        let prepared = PreparedTransfer {
            phase,
            source,
            target,
            source_snapshots,
            target_snapshots,
            decision: None,
            applied: None,
        };
        let target_store = self
            .stores
            .get(&target.key)
            .ok_or(RegionEntityStoreError::StaleLease)?;
        if prepared.target_snapshots.iter().any(|snapshot| {
            target_store.contains(snapshot.id) || target_store.contains_uuid(snapshot.uuid)
        }) {
            return Err(RegionEntityStoreError::TargetConflict);
        }
        self.transfers.insert(transfer, prepared);
        for id in group_ids {
            self.in_flight_transfers.insert(id, transfer);
        }
        Ok(transfer)
    }

    pub fn apply_kinematics(
        &mut self,
        phase: RegionPhase,
        tick: u64,
        state: EntityKinematics,
    ) -> Result<RegionalKinematicsApply, RegionEntityStoreError> {
        if !state.is_finite() {
            return Err(RegionEntityStoreError::InvalidKinematics);
        }
        let source_key = self
            .region_for(state.id)
            .ok_or(RegionEntityStoreError::UnknownEntity)?;
        let source = self
            .ownership
            .lease(source_key)
            .ok_or(RegionEntityStoreError::StaleLease)?;
        self.validate_access(phase, source)?;
        if self.in_flight_transfers.contains_key(&state.id) {
            return Err(RegionEntityStoreError::TransferConflict);
        }
        let target_key = RegionKey::from_position(state.position)
            .ok_or(RegionEntityStoreError::InvalidKinematics)?;
        if target_key == source_key {
            let applied = self
                .stores
                .get_mut(&source_key)
                .ok_or(RegionEntityStoreError::StaleLease)?
                .apply_kinematics([state]);
            return if applied == 1 {
                Ok(RegionalKinematicsApply::AppliedLocal)
            } else {
                Err(RegionEntityStoreError::UnknownEntity)
            };
        }
        let target = self
            .ownership
            .lease(target_key)
            .ok_or(RegionEntityStoreError::Ownership(
                RegionOwnershipError::UnknownRegion,
            ))?;
        let transfer = self.prepare_transfer(phase, source, target, tick, state)?;
        Ok(RegionalKinematicsApply::PreparedTransfer(transfer))
    }

    pub fn set_velocity(
        &mut self,
        phase: RegionPhase,
        id: EntityId,
        velocity: Vec3,
    ) -> Result<bool, RegionEntityStoreError> {
        Ok(self
            .store_for_entity_mutation(phase, id)?
            .set_velocity(id, velocity))
    }

    pub fn set_animal_state(
        &mut self,
        phase: RegionPhase,
        id: EntityId,
        animal: AnimalBreedingState,
    ) -> Result<bool, RegionEntityStoreError> {
        Ok(self
            .store_for_entity_mutation(phase, id)?
            .set_animal_state(id, animal))
    }

    pub fn set_goal(
        &mut self,
        phase: RegionPhase,
        id: EntityId,
        goal: GoalState,
    ) -> Result<bool, RegionEntityStoreError> {
        if goal_reference(&goal).is_some_and(|target| !self.locations.contains_key(&target)) {
            return Err(RegionEntityStoreError::CrossRegionReference);
        }
        Ok(self
            .store_for_entity_mutation(phase, id)?
            .set_goal(id, goal))
    }

    pub fn prepare_goal_tick_with_pathing_for_ids(
        &mut self,
        phase: RegionPhase,
        tick: u64,
        active_ids: &HashSet<EntityId>,
    ) -> Result<RegionalPreparedGoalTick, RegionEntityStoreError> {
        if !self.ownership.validate_phase(phase) {
            return Err(RegionEntityStoreError::StalePhase);
        }

        let mut ids_by_region = BTreeMap::<RegionKey, HashSet<EntityId>>::new();
        for id in active_ids {
            if let Some(key) = self.locations.get(id).copied() {
                ids_by_region.entry(key).or_default().insert(*id);
            }
        }
        for key in ids_by_region.keys() {
            let lease = self
                .ownership
                .lease(*key)
                .ok_or(RegionEntityStoreError::StaleLease)?;
            self.validate_access(phase, lease)?;
        }

        let captured = self.capture_follow_targets(&ids_by_region);

        let mut batches = BTreeMap::new();
        for (key, ids) in ids_by_region {
            let prepared = self
                .stores
                .get_mut(&key)
                .expect("regional goal stores were preflighted")
                .prepare_goal_tick_with_pathing_for_ids(tick, &ids);
            batches.insert(key, prepared);
        }
        let lease_keys = batches
            .keys()
            .copied()
            .chain(captured.sources.values().map(|source| source.region))
            .collect::<BTreeSet<_>>();
        Ok(RegionalPreparedGoalTick {
            authority: self.authority,
            phase,
            leases: lease_keys
                .into_iter()
                .map(|key| {
                    (
                        key,
                        self.ownership
                            .lease(key)
                            .expect("prepared regional goal lease"),
                    )
                })
                .collect(),
            batches,
            follow_targets: captured.remote_by_region,
            follow_target_sources: captured.sources,
            expected_missing_follow_targets: captured.expected_missing_follow_targets,
            goal_inputs: captured.inputs,
        })
    }

    pub fn apply_prepared_goal_tick(
        &mut self,
        phase: RegionPhase,
        resolved: RegionalResolvedGoalTick,
    ) -> Result<GoalTickStats, RegionEntityStoreError> {
        if resolved.authority != self.authority {
            return Err(RegionEntityStoreError::StaleLease);
        }
        if resolved.phase != phase || !self.ownership.validate_phase(phase) {
            return Err(RegionEntityStoreError::StalePhase);
        }
        if resolved
            .expected_missing_follow_targets
            .iter()
            .any(|target| self.locations.contains_key(target))
        {
            return Err(RegionEntityStoreError::SourceChanged);
        }
        if !self.follow_target_sources_match(&resolved.follow_target_sources) {
            return Err(RegionEntityStoreError::SourceChanged);
        }
        if !self.goal_inputs_match(&resolved.goal_inputs) {
            return Err(RegionEntityStoreError::SourceChanged);
        }
        for (&key, &lease) in &resolved.leases {
            if lease.key != key {
                return Err(RegionEntityStoreError::StaleLease);
            }
            self.validate_access(phase, lease)?;
        }

        let mut total = GoalTickStats::default();
        let mut follow_targets = resolved.follow_targets;
        for (key, batch) in resolved.batches {
            let targets = follow_targets.remove(&key).unwrap_or_default();
            let stats = self
                .stores
                .get_mut(&key)
                .expect("regional goal stores were preflighted")
                .apply_prepared_goal_tick_with_follow_targets(batch, &targets);
            add_goal_tick_stats(&mut total, stats);
        }
        Ok(total)
    }

    pub fn damage(
        &mut self,
        phase: RegionPhase,
        id: EntityId,
        amount: f32,
    ) -> Result<Option<EntityDamage>, RegionEntityStoreError> {
        Ok(self
            .store_for_entity_mutation(phase, id)?
            .damage(id, amount))
    }

    pub fn decide_transfer(
        &mut self,
        phase: RegionPhase,
        transfer: TransferId,
        decision: TransferDecision,
    ) -> Result<(), RegionEntityStoreError> {
        if !self.ownership.validate_phase(phase) {
            return Err(RegionEntityStoreError::StalePhase);
        }
        let prepared = self
            .transfers
            .get_mut(&transfer)
            .ok_or(RegionEntityStoreError::UnknownTransfer)?;
        if prepared.phase != phase {
            return Err(RegionEntityStoreError::StalePhase);
        }
        match prepared.decision {
            None => prepared.decision = Some(decision),
            Some(existing) if existing == decision => {}
            Some(_) => return Err(RegionEntityStoreError::DecisionConflict),
        }
        Ok(())
    }

    pub fn apply_transfer(
        &mut self,
        phase: RegionPhase,
        transfer: TransferId,
    ) -> Result<TransferApply, RegionEntityStoreError> {
        if !self.ownership.validate_phase(phase) {
            return Err(RegionEntityStoreError::StalePhase);
        }
        let prepared = self
            .transfers
            .get(&transfer)
            .cloned()
            .ok_or(RegionEntityStoreError::UnknownTransfer)?;
        if prepared.phase != phase {
            return Err(RegionEntityStoreError::StalePhase);
        }
        if let Some(applied) = prepared.applied {
            return Ok(applied);
        }
        let decision = prepared
            .decision
            .ok_or(RegionEntityStoreError::TransferUndecided)?;
        if decision == TransferDecision::Reject {
            self.finish_transfer(transfer, TransferApply::Rejected);
            return Ok(TransferApply::Rejected);
        }

        if prepared
            .source_snapshots
            .iter()
            .any(|snapshot| self.locations.get(&snapshot.id).copied() != Some(prepared.source.key))
        {
            return Err(RegionEntityStoreError::WrongSourceRegion);
        }
        let source_store = self
            .stores
            .get(&prepared.source.key)
            .ok_or(RegionEntityStoreError::StaleLease)?;
        if prepared
            .source_snapshots
            .iter()
            .any(|snapshot| source_store.snapshot(snapshot.id).as_ref() != Some(snapshot))
        {
            return Err(RegionEntityStoreError::SourceChanged);
        }
        let target_store = self
            .stores
            .get(&prepared.target.key)
            .ok_or(RegionEntityStoreError::StaleLease)?;
        if prepared.target_snapshots.iter().any(|snapshot| {
            target_store.contains(snapshot.id) || target_store.contains_uuid(snapshot.uuid)
        }) {
            return Err(RegionEntityStoreError::TargetConflict);
        }

        let source_ids = prepared
            .source_snapshots
            .iter()
            .map(|snapshot| snapshot.id)
            .collect::<Vec<_>>();
        for &id in &source_ids {
            let removed = self
                .stores
                .get_mut(&prepared.source.key)
                .and_then(|store| store.remove(id));
            if removed.is_none() {
                return Err(RegionEntityStoreError::UnknownEntity);
            }
        }
        let inserted = self
            .stores
            .get_mut(&prepared.target.key)
            .ok_or(RegionEntityStoreError::StaleLease)?
            .insert_authoritative_snapshots_batch(prepared.target_snapshots.clone());
        if !inserted {
            let restored = self
                .stores
                .get_mut(&prepared.source.key)
                .is_some_and(|store| {
                    store.insert_authoritative_snapshots_batch(prepared.source_snapshots.clone())
                });
            assert!(
                restored,
                "validated transfer rollback must restore source authority"
            );
            return Err(RegionEntityStoreError::TargetConflict);
        }
        for id in source_ids {
            self.locations.insert(id, prepared.target.key);
        }
        self.finish_transfer(transfer, TransferApply::Committed);
        Ok(TransferApply::Committed)
    }

    pub fn spawn_authoritative(
        &mut self,
        phase: RegionPhase,
        lease: RegionLease,
        entity: SpawnEntity,
    ) -> Result<EntityId, RegionEntityStoreError> {
        self.validate_access(phase, lease)?;
        if RegionKey::from_position(entity.position) != Some(lease.key) {
            return Err(RegionEntityStoreError::WrongSpawnRegion);
        }
        let passenger = entity_vehicle_reference(&entity);
        if passenger.is_some_and(|id| self.in_flight_transfers.contains_key(&id)) {
            return Err(RegionEntityStoreError::TransferConflict);
        }
        if passenger.is_some_and(|id| self.region_for(id) != Some(lease.key))
            || goal_reference(&entity.goal)
                .is_some_and(|target| !self.locations.contains_key(&target))
        {
            return Err(RegionEntityStoreError::CrossRegionReference);
        }
        let id = self.next_available_id()?;
        let uuid = entity.uuid.unwrap_or_else(|| deterministic_uuid(id));
        if self.uuids.contains_key(&uuid) {
            return Err(RegionEntityStoreError::DuplicateUuid);
        }
        let snapshot = snapshot_from_spawn(id, uuid, entity);
        let store = self
            .stores
            .get_mut(&lease.key)
            .ok_or(RegionEntityStoreError::StaleLease)?;
        if !store.insert_authoritative_snapshot(snapshot) {
            return Err(RegionEntityStoreError::TargetConflict);
        }
        self.locations.insert(id, lease.key);
        self.uuids.insert(uuid, id);
        self.next_id = id.0;
        Ok(id)
    }

    pub fn spawn_authoritative_batch(
        &mut self,
        phase: RegionPhase,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Result<Vec<EntityId>, RegionEntityStoreError> {
        let mut pending = Vec::new();
        let mut pending_ids = HashSet::new();
        let mut pending_uuids = HashSet::new();
        let mut cursor = self.next_id;
        for entity in entities {
            let key = RegionKey::from_position(entity.position)
                .ok_or(RegionEntityStoreError::WrongSpawnRegion)?;
            let lease = self
                .ownership
                .lease(key)
                .ok_or(RegionEntityStoreError::Ownership(
                    RegionOwnershipError::UnknownRegion,
                ))?;
            self.validate_access(phase, lease)?;
            let id = self.next_available_id_after(cursor, &pending_ids)?;
            let uuid = entity.uuid.unwrap_or_else(|| deterministic_uuid(id));
            if self.uuids.contains_key(&uuid) || !pending_uuids.insert(uuid) {
                return Err(RegionEntityStoreError::DuplicateUuid);
            }
            pending_ids.insert(id);
            cursor = id.0;
            pending.push(PendingInsert {
                key,
                snapshot: snapshot_from_spawn(id, uuid, entity),
            });
        }
        let ids = pending
            .iter()
            .map(|entity| entity.snapshot.id)
            .collect::<Vec<_>>();
        self.insert_prepared_snapshots(phase, pending)?;
        Ok(ids)
    }

    pub fn insert_authoritative_snapshots(
        &mut self,
        phase: RegionPhase,
        snapshots: impl IntoIterator<Item = EntitySnapshot>,
    ) -> Result<usize, RegionEntityStoreError> {
        let mut pending = Vec::new();
        let mut pending_ids = HashSet::new();
        let mut pending_uuids = HashSet::new();
        for snapshot in snapshots {
            if self.contains(snapshot.id) || !pending_ids.insert(snapshot.id) {
                return Err(RegionEntityStoreError::TargetConflict);
            }
            if self.contains_uuid(snapshot.uuid) || !pending_uuids.insert(snapshot.uuid) {
                return Err(RegionEntityStoreError::DuplicateUuid);
            }
            let key = RegionKey::from_position(snapshot.position)
                .ok_or(RegionEntityStoreError::WrongSpawnRegion)?;
            let lease = self
                .ownership
                .lease(key)
                .ok_or(RegionEntityStoreError::Ownership(
                    RegionOwnershipError::UnknownRegion,
                ))?;
            self.validate_access(phase, lease)?;
            pending.push(PendingInsert { key, snapshot });
        }
        let count = pending.len();
        self.insert_prepared_snapshots(phase, pending)?;
        Ok(count)
    }

    pub fn remove(
        &mut self,
        phase: RegionPhase,
        lease: RegionLease,
        id: EntityId,
    ) -> Result<EntitySnapshot, RegionEntityStoreError> {
        self.validate_access(phase, lease)?;
        if self.in_flight_transfers.contains_key(&id) {
            return Err(RegionEntityStoreError::TransferConflict);
        }
        if self.locations.get(&id).copied() != Some(lease.key) {
            return Err(RegionEntityStoreError::WrongSourceRegion);
        }
        let removed = self
            .stores
            .get_mut(&lease.key)
            .and_then(|store| store.remove(id))
            .ok_or(RegionEntityStoreError::UnknownEntity)?;
        self.locations.remove(&id);
        self.uuids.remove(&removed.uuid);
        Ok(removed)
    }

    #[must_use]
    pub fn snapshot(&self, id: EntityId) -> Option<EntitySnapshot> {
        let key = self.locations.get(&id)?;
        self.stores.get(key)?.snapshot(id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.locations.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.locations.is_empty()
    }

    #[must_use]
    pub fn contains(&self, id: EntityId) -> bool {
        self.locations.contains_key(&id)
    }

    #[must_use]
    pub fn contains_uuid(&self, uuid: Uuid) -> bool {
        self.uuids.contains_key(&uuid)
    }

    #[must_use]
    pub fn motion_state(&self, id: EntityId) -> Option<EntityMotionState> {
        let key = self.locations.get(&id)?;
        self.stores.get(key)?.motion_state(id)
    }

    pub fn snapshots(&self) -> impl Iterator<Item = EntitySnapshot> + '_ {
        self.locations.keys().filter_map(|&id| self.snapshot(id))
    }

    #[cfg(any(test, feature = "shadow-compare"))]
    #[must_use]
    pub fn shadow_comparison_stats(&self) -> &ShadowComparisonStats {
        &self.shadow_stats
    }

    #[cfg(any(test, feature = "shadow-compare"))]
    pub fn compare_shadow(
        &mut self,
        tick: u64,
        stage: ShadowStage,
    ) -> Result<ShadowComparison, Box<ShadowDivergence>> {
        let mut batch = RegionalShadowComparisonBatch::new(tick, stage);
        for (&key, store) in &mut self.stores {
            batch
                .compare_region(key, store)
                .expect("regional stores have unique keys");
        }
        self.record_shadow_comparison(batch.finish())
    }

    #[cfg(any(test, feature = "shadow-compare"))]
    pub fn record_shadow_comparison(
        &mut self,
        outcome: RegionalShadowComparisonOutcome,
    ) -> Result<ShadowComparison, Box<ShadowDivergence>> {
        record_regional_shadow_comparison(&mut self.shadow_stats, outcome)
    }

    pub fn visit_simulation_entities(&self, mut visitor: impl FnMut(EntityView<'_>)) {
        for &id in self.locations.keys() {
            self.visit_entity(id, &mut visitor);
        }
    }

    pub fn visit_simulation_entities_for_ids(
        &self,
        ids: &HashSet<EntityId>,
        mut visitor: impl FnMut(EntityView<'_>),
    ) {
        let mut ordered = ids
            .iter()
            .filter(|id| self.locations.contains_key(id))
            .copied()
            .collect::<Vec<_>>();
        ordered.sort_unstable();
        for id in ordered {
            self.visit_entity(id, &mut visitor);
        }
    }

    pub fn visit_breeding_tick_entities(&self, mut visitor: impl FnMut(EntityView<'_>)) {
        let mut ids = Vec::new();
        for store in self.stores.values() {
            store.visit_breeding_tick_entities(|entity| ids.push(entity.id));
        }
        ids.sort_unstable();
        for id in ids {
            self.visit_entity(id, &mut visitor);
        }
    }

    pub fn visit_sheep_entities_for_ids(
        &self,
        ids: &HashSet<EntityId>,
        mut visitor: impl FnMut(EntityView<'_>),
    ) {
        let mut ordered = Vec::new();
        for store in self.stores.values() {
            store.visit_sheep_entities_for_ids(ids, |entity| ordered.push(entity.id));
        }
        ordered.sort_unstable();
        for id in ordered {
            self.visit_entity(id, &mut visitor);
        }
    }

    #[must_use]
    pub fn region_for(&self, id: EntityId) -> Option<RegionKey> {
        self.locations.get(&id).copied()
    }

    #[must_use]
    pub fn region_len(&self, key: RegionKey) -> usize {
        self.stores.get(&key).map_or(0, EntityStore::len)
    }

    fn validate_access(
        &self,
        phase: RegionPhase,
        lease: RegionLease,
    ) -> Result<(), RegionEntityStoreError> {
        if !self.ownership.validate_phase(phase) {
            return Err(RegionEntityStoreError::StalePhase);
        }
        if !self.ownership.validate(lease) || !self.stores.contains_key(&lease.key) {
            return Err(RegionEntityStoreError::StaleLease);
        }
        self.ownership.validate_lane(phase, lease.lane)?;
        Ok(())
    }

    fn has_incoming_cross_region_reference(
        &self,
        targets: &HashSet<EntityId>,
        destination: RegionKey,
    ) -> bool {
        self.locations.iter().any(|(&id, &region)| {
            !targets.contains(&id)
                && region != destination
                && self
                    .stores
                    .get(&region)
                    .and_then(|store| store.snapshot(id))
                    .and_then(|entity| snapshot_vehicle_reference(&entity))
                    .is_some_and(|referenced| targets.contains(&referenced))
        })
    }

    fn vehicle_group_snapshots(
        &self,
        source: RegionKey,
        root: EntityId,
    ) -> Result<Vec<EntitySnapshot>, RegionEntityStoreError> {
        let store = self
            .stores
            .get(&source)
            .ok_or(RegionEntityStoreError::StaleLease)?;
        if !store.contains(root) {
            return Err(RegionEntityStoreError::UnknownEntity);
        }
        let snapshots = store.snapshots().collect::<Vec<_>>();
        let mut group = HashSet::from([root]);
        loop {
            let before = group.len();
            for snapshot in &snapshots {
                let passenger = snapshot.vehicle.and_then(|vehicle| vehicle.passenger);
                if group.contains(&snapshot.id) {
                    if let Some(passenger) = passenger {
                        group.insert(passenger);
                    }
                } else if passenger.is_some_and(|passenger| group.contains(&passenger)) {
                    group.insert(snapshot.id);
                }
            }
            if group.len() == before {
                break;
            }
        }
        let group = snapshots
            .into_iter()
            .filter(|snapshot| group.contains(&snapshot.id))
            .collect::<Vec<_>>();
        if group.len() == 1 && group[0].id != root {
            return Err(RegionEntityStoreError::UnknownEntity);
        }
        Ok(group)
    }

    fn capture_follow_targets(
        &self,
        ids_by_region: &BTreeMap<RegionKey, HashSet<EntityId>>,
    ) -> CapturedFollowTargets {
        let mut remote_by_region = BTreeMap::new();
        let mut sources = BTreeMap::new();
        let mut expected_missing_follow_targets = HashSet::new();
        let mut inputs = BTreeMap::new();
        for (&region, ids) in ids_by_region {
            let mut remote = HashMap::new();
            for &id in ids {
                let Some(follower) = self.snapshot(id) else {
                    continue;
                };
                inputs.insert(id, follower.clone());
                let Some(target) = goal_reference(&follower.goal) else {
                    continue;
                };
                let Some(target_region) = self.region_for(target) else {
                    expected_missing_follow_targets.insert(target);
                    continue;
                };
                let Some(target_snapshot) = self.snapshot(target) else {
                    expected_missing_follow_targets.insert(target);
                    continue;
                };
                sources.insert(
                    target,
                    RegionalFollowTargetSource {
                        region: target_region,
                        snapshot: target_snapshot.clone(),
                    },
                );
                if target_region != region {
                    remote.insert(target, target_snapshot.position);
                }
            }
            if !remote.is_empty() {
                remote_by_region.insert(region, remote);
            }
        }
        CapturedFollowTargets {
            remote_by_region,
            sources,
            expected_missing_follow_targets,
            inputs,
        }
    }

    fn follow_target_sources_match(
        &self,
        expected: &BTreeMap<EntityId, RegionalFollowTargetSource>,
    ) -> bool {
        expected.iter().all(|(&id, expected)| {
            self.region_for(id) == Some(expected.region)
                && self.snapshot(id).as_ref() == Some(&expected.snapshot)
        })
    }

    fn goal_inputs_match(&self, expected: &BTreeMap<EntityId, EntitySnapshot>) -> bool {
        expected
            .iter()
            .all(|(&id, expected)| self.snapshot(id).as_ref() == Some(expected))
    }

    fn store_for_entity_mutation(
        &mut self,
        phase: RegionPhase,
        id: EntityId,
    ) -> Result<&mut EntityStore, RegionEntityStoreError> {
        if self.in_flight_transfers.contains_key(&id) {
            return Err(RegionEntityStoreError::TransferConflict);
        }
        let key = self
            .region_for(id)
            .ok_or(RegionEntityStoreError::UnknownEntity)?;
        let lease = self
            .ownership
            .lease(key)
            .ok_or(RegionEntityStoreError::StaleLease)?;
        self.validate_access(phase, lease)?;
        self.stores
            .get_mut(&key)
            .ok_or(RegionEntityStoreError::StaleLease)
    }

    fn next_available_id(&self) -> Result<EntityId, RegionEntityStoreError> {
        self.next_available_id_after(self.next_id, &HashSet::new())
    }

    fn next_available_id_after(
        &self,
        cursor: i32,
        pending: &HashSet<EntityId>,
    ) -> Result<EntityId, RegionEntityStoreError> {
        let mut next_id = cursor.max(0);
        loop {
            next_id = next_id
                .checked_add(1)
                .ok_or(RegionEntityStoreError::IdExhausted)?;
            let id = EntityId(next_id);
            if !self.locations.contains_key(&id) && !pending.contains(&id) {
                return Ok(id);
            }
        }
    }

    fn insert_prepared_snapshots(
        &mut self,
        phase: RegionPhase,
        pending: Vec<PendingInsert>,
    ) -> Result<(), RegionEntityStoreError> {
        let pending_locations = pending
            .iter()
            .map(|entity| (entity.snapshot.id, entity.key))
            .collect::<HashMap<_, _>>();
        for entity in &pending {
            let lease = self
                .ownership
                .lease(entity.key)
                .ok_or(RegionEntityStoreError::StaleLease)?;
            self.validate_access(phase, lease)?;
            let passenger = snapshot_vehicle_reference(&entity.snapshot);
            if passenger.is_some_and(|id| self.in_flight_transfers.contains_key(&id)) {
                return Err(RegionEntityStoreError::TransferConflict);
            }
            if passenger.is_some_and(|id| {
                self.region_for(id)
                    .or_else(|| pending_locations.get(&id).copied())
                    != Some(entity.key)
            }) || goal_reference(&entity.snapshot.goal).is_some_and(|target| {
                !self.locations.contains_key(&target) && !pending_locations.contains_key(&target)
            }) {
                return Err(RegionEntityStoreError::CrossRegionReference);
            }
            let store = self
                .stores
                .get(&entity.key)
                .ok_or(RegionEntityStoreError::StaleLease)?;
            if store.contains(entity.snapshot.id) || store.contains_uuid(entity.snapshot.uuid) {
                return Err(RegionEntityStoreError::TargetConflict);
            }
        }

        let mut grouped = BTreeMap::<RegionKey, Vec<EntitySnapshot>>::new();
        for entity in &pending {
            grouped
                .entry(entity.key)
                .or_default()
                .push(entity.snapshot.clone());
        }
        let mut inserted = Vec::with_capacity(pending.len());
        for (key, snapshots) in grouped {
            let ids = snapshots
                .iter()
                .map(|snapshot| snapshot.id)
                .collect::<Vec<_>>();
            let success = self
                .stores
                .get_mut(&key)
                .ok_or(RegionEntityStoreError::StaleLease)?
                .insert_authoritative_snapshots_batch(snapshots);
            if !success {
                for &(key, id) in &inserted {
                    let _ = self.stores.get_mut(&key).and_then(|store| store.remove(id));
                }
                return Err(RegionEntityStoreError::TargetConflict);
            }
            inserted.extend(ids.into_iter().map(|id| (key, id)));
        }
        for entity in pending {
            self.next_id = self.next_id.max(entity.snapshot.id.0);
            self.locations.insert(entity.snapshot.id, entity.key);
            self.uuids.insert(entity.snapshot.uuid, entity.snapshot.id);
        }
        Ok(())
    }

    fn visit_entity(&self, id: EntityId, visitor: &mut impl FnMut(EntityView<'_>)) {
        let Some(key) = self.locations.get(&id) else {
            return;
        };
        let Some(store) = self.stores.get(key) else {
            return;
        };
        store.visit_simulation_entities_for_ids(&HashSet::from([id]), visitor);
    }

    fn finish_transfer(&mut self, transfer: TransferId, applied: TransferApply) {
        let entity_ids = self
            .transfers
            .get_mut(&transfer)
            .map(|prepared| {
                prepared.applied = Some(applied);
                prepared
                    .source_snapshots
                    .iter()
                    .map(|snapshot| snapshot.id)
                    .collect::<Vec<_>>()
            })
            .expect("known transfer");
        for id in entity_ids {
            self.in_flight_transfers.remove(&id);
        }
    }

    fn transfer_entity_ids(&self, transfer: TransferId) -> Vec<EntityId> {
        self.transfers
            .get(&transfer)
            .map(|prepared| {
                prepared
                    .source_snapshots
                    .iter()
                    .map(|snapshot| snapshot.id)
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(any(test, feature = "shadow-compare"))]
fn record_regional_shadow_comparison(
    stats: &mut ShadowComparisonStats,
    outcome: RegionalShadowComparisonOutcome,
) -> Result<ShadowComparison, Box<ShadowDivergence>> {
    let RegionalShadowComparisonOutcome {
        comparison,
        first_divergence,
    } = outcome;
    stats.comparisons = stats.comparisons.saturating_add(1);
    stats.compared_entities = stats
        .compared_entities
        .saturating_add(comparison.compared_entities as u64);
    stats.compared_events = stats
        .compared_events
        .saturating_add(comparison.compared_events as u64);

    if let Some(divergence) = first_divergence {
        if stats.first_divergence.is_none() {
            stats.first_divergence = Some((*divergence).clone());
        }
        return Err(divergence);
    }
    Ok(comparison)
}

#[derive(Debug)]
pub struct RegionalEntityAuthority {
    regions: RegionalEntityStore,
    lane: usize,
    transfer_tick: u64,
}

struct SingleLanePhase<'a> {
    regions: &'a mut RegionalEntityStore,
    phase: RegionPhase,
    lane: usize,
    finished: bool,
}

impl SingleLanePhase<'_> {
    fn finish(mut self) -> Result<(), RegionEntityStoreError> {
        if !self.regions.stores.is_empty() {
            self.regions.acknowledge_lane(self.phase, self.lane)?;
        }
        self.regions.finish_phase(self.phase)?;
        self.finished = true;
        Ok(())
    }
}

impl Drop for SingleLanePhase<'_> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if !self.regions.stores.is_empty() {
            let _ = self.regions.acknowledge_lane(self.phase, self.lane);
        }
        let _ = self.regions.finish_phase(self.phase);
    }
}

impl RegionalEntityAuthority {
    #[must_use]
    pub fn with_next_id(next_id: i32) -> Self {
        let mut regions = RegionalEntityStore::new();
        regions.next_id = next_id.max(0);
        Self {
            regions,
            lane: 0,
            transfer_tick: 0,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    #[must_use]
    pub fn contains(&self, id: EntityId) -> bool {
        self.regions.contains(id)
    }

    #[must_use]
    pub fn contains_uuid(&self, uuid: Uuid) -> bool {
        self.regions.contains_uuid(uuid)
    }

    #[must_use]
    pub fn snapshot(&self, id: EntityId) -> Option<EntitySnapshot> {
        self.regions.snapshot(id)
    }

    #[must_use]
    pub fn motion_state(&self, id: EntityId) -> Option<EntityMotionState> {
        self.regions.motion_state(id)
    }

    pub fn snapshots(&self) -> impl Iterator<Item = EntitySnapshot> + '_ {
        self.regions.snapshots()
    }

    #[must_use]
    pub fn region_len(&self, key: RegionKey) -> usize {
        self.regions.region_len(key)
    }

    #[must_use]
    pub fn parallel_kinematics_batch_count(&self, states: &[EntityKinematics]) -> usize {
        if states.len() < PARALLEL_KINEMATICS_MIN_STATES {
            return 0;
        }
        let mut ids = HashSet::with_capacity(states.len());
        if states.iter().any(|state| !ids.insert(state.id)) {
            return 0;
        }
        states
            .iter()
            .filter_map(|state| {
                if !state.is_finite() {
                    return None;
                }
                let source = self.regions.region_for(state.id)?;
                (RegionKey::from_position(state.position) == Some(source)).then_some(source)
            })
            .collect::<BTreeSet<_>>()
            .len()
    }

    pub fn spawn(&mut self, entity: SpawnEntity) -> EntityId {
        self.spawn_authoritative(entity)
    }

    pub fn spawn_authoritative(&mut self, entity: SpawnEntity) -> EntityId {
        let key = self.ensure_position_region(entity.position);
        self.with_phase(|regions, phase| {
            let lease = regions.ownership.lease(key).expect("assigned spawn region");
            regions
                .spawn_authoritative(phase, lease, entity)
                .expect("validated production entity spawn")
        })
    }

    pub fn spawn_authoritative_batch(
        &mut self,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Vec<EntityId> {
        let entities = entities.into_iter().collect::<Vec<_>>();
        for entity in &entities {
            self.ensure_position_region(entity.position);
        }
        self.with_phase(|regions, phase| {
            regions
                .spawn_authoritative_batch(phase, entities)
                .expect("validated production entity batch")
        })
    }

    pub fn insert_authoritative_snapshot(&mut self, snapshot: EntitySnapshot) -> bool {
        self.ensure_position_region(snapshot.position);
        self.with_phase(|regions, phase| {
            regions
                .insert_authoritative_snapshots(phase, [snapshot])
                .is_ok()
        })
    }

    pub fn insert_authoritative_snapshots_batch(
        &mut self,
        snapshots: impl IntoIterator<Item = EntitySnapshot>,
    ) -> bool {
        let snapshots = snapshots.into_iter().collect::<Vec<_>>();
        for snapshot in &snapshots {
            self.ensure_position_region(snapshot.position);
        }
        let expected = snapshots.len();
        self.with_phase(|regions, phase| {
            regions.insert_authoritative_snapshots(phase, snapshots) == Ok(expected)
        })
    }

    pub fn set_animal_state(&mut self, id: EntityId, animal: AnimalBreedingState) -> bool {
        self.with_entity_phase(id, |regions, phase| {
            regions.set_animal_state(phase, id, animal).unwrap_or(false)
        })
        .unwrap_or(false)
    }

    pub fn set_animal_states(
        &mut self,
        states: impl IntoIterator<Item = (EntityId, AnimalBreedingState)>,
    ) -> usize {
        let states = states.into_iter().collect::<Vec<_>>();
        if states.is_empty() {
            return 0;
        }
        self.with_phase(|regions, phase| {
            states
                .into_iter()
                .filter(|(id, animal)| {
                    regions
                        .set_animal_state(phase, *id, *animal)
                        .unwrap_or(false)
                })
                .count()
        })
    }

    pub fn set_animal_states_if_current(
        &mut self,
        states: impl IntoIterator<Item = (EntitySnapshot, AnimalBreedingState)>,
    ) -> bool {
        let states = states.into_iter().collect::<Vec<_>>();
        if states.is_empty() {
            return true;
        }
        let mut entity_ids = HashSet::with_capacity(states.len());
        if states.iter().any(|(expected, _)| {
            expected.animal.is_none()
                || !entity_ids.insert(expected.id)
                || self.regions.in_flight_transfers.contains_key(&expected.id)
                || self.regions.snapshot(expected.id).as_ref() != Some(expected)
        }) {
            return false;
        }

        self.with_phase(|regions, phase| {
            states.into_iter().all(|(expected, animal)| {
                regions
                    .set_animal_state(phase, expected.id, animal)
                    .unwrap_or(false)
            })
        })
    }

    pub fn set_goal(&mut self, id: EntityId, goal: GoalState) -> bool {
        self.with_entity_phase(id, |regions, phase| {
            regions.set_goal(phase, id, goal).unwrap_or(false)
        })
        .unwrap_or(false)
    }

    pub fn set_goals(&mut self, goals: impl IntoIterator<Item = (EntityId, GoalState)>) -> usize {
        let goals = goals.into_iter().collect::<Vec<_>>();
        if goals.is_empty() {
            return 0;
        }
        self.with_phase(|regions, phase| {
            goals
                .into_iter()
                .filter(|(id, goal)| regions.set_goal(phase, *id, goal.clone()).unwrap_or(false))
                .count()
        })
    }

    pub fn set_item_stack(&mut self, id: EntityId, stack: Option<EntityItemStack>) -> bool {
        self.with_entity_phase(id, |regions, phase| {
            regions
                .store_for_entity_mutation(phase, id)
                .is_ok_and(|store| store.set_item_stack(id, stack))
        })
        .unwrap_or(false)
    }

    pub fn set_velocity(&mut self, id: EntityId, velocity: Vec3) -> bool {
        self.with_entity_phase(id, |regions, phase| {
            regions.set_velocity(phase, id, velocity).unwrap_or(false)
        })
        .unwrap_or(false)
    }

    pub fn set_position(&mut self, id: EntityId, position: Vec3) -> bool {
        let Some(mut motion) = self.motion_state(id) else {
            return false;
        };
        if !position.is_finite() {
            return false;
        }
        self.ensure_position_region(position);
        motion.position = position;
        self.apply_kinematics([EntityKinematics {
            id,
            position: motion.position,
            rotation: motion.rotation,
            velocity: motion.velocity,
            on_ground: motion.on_ground,
        }]) == 1
    }

    pub fn damage(&mut self, id: EntityId, amount: f32) -> Option<EntityDamage> {
        self.with_entity_phase(id, |regions, phase| {
            regions.damage(phase, id, amount).ok().flatten()
        })
        .flatten()
    }

    pub fn remove(&mut self, id: EntityId) -> Option<EntitySnapshot> {
        let key = self.regions.region_for(id)?;
        self.with_phase(|regions, phase| {
            let lease = regions.ownership.lease(key).expect("known entity region");
            regions.remove(phase, lease, id).ok()
        })
    }

    pub fn apply_kinematics(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
    ) -> usize {
        self.apply_kinematics_serial(states.into_iter().collect())
            .len()
    }

    pub fn apply_kinematics_authoritative(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
    ) -> Vec<EntityKinematics> {
        let applied = self.apply_kinematics_serial(states.into_iter().collect());
        self.authoritative_kinematics(applied)
    }

    pub fn apply_kinematics_parallel(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
        max_workers: usize,
    ) -> usize {
        self.apply_kinematics_parallel_inner(states.into_iter().collect(), max_workers, &|_| {})
            .len()
    }

    pub fn apply_kinematics_parallel_authoritative(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
        max_workers: usize,
    ) -> Vec<EntityKinematics> {
        let applied = self.apply_kinematics_parallel_inner(
            states.into_iter().collect(),
            max_workers,
            &|_| {},
        );
        self.authoritative_kinematics(applied)
    }

    #[cfg(test)]
    fn apply_kinematics_parallel_with_probe(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
        max_workers: usize,
        before_region: &(dyn Fn(RegionKey) + Sync),
    ) -> usize {
        self.apply_kinematics_parallel_inner(
            states.into_iter().collect(),
            max_workers,
            before_region,
        )
        .len()
    }

    fn apply_kinematics_parallel_inner(
        &mut self,
        states: Vec<EntityKinematics>,
        max_workers: usize,
        before_region: &(dyn Fn(RegionKey) + Sync),
    ) -> Vec<EntityId> {
        if max_workers <= 1 {
            return self.apply_kinematics_serial(states);
        }
        let states = states
            .into_iter()
            .filter(|state| state.is_finite())
            .collect::<Vec<_>>();
        let mut unique_ids = HashSet::with_capacity(states.len());
        if states.iter().any(|state| !unique_ids.insert(state.id)) {
            return self.apply_kinematics_serial(states);
        }
        for state in &states {
            self.ensure_position_region(state.position);
        }

        let mut crossing_leaders = HashMap::new();
        for state in &states {
            let Some(source) = self.regions.region_for(state.id) else {
                continue;
            };
            if RegionKey::from_position(state.position) == Some(source) {
                continue;
            }
            if let Ok(group) = self.regions.vehicle_group_snapshots(source, state.id)
                && let Some(leader) = vehicle_group_leader(&group)
            {
                for member in group {
                    crossing_leaders.insert(member.id, leader);
                }
            }
        }

        let mut ordered = states;
        ordered.sort_by_key(|state| {
            let leader = crossing_leaders.get(&state.id).copied().unwrap_or(state.id);
            (leader, state.id != leader)
        });
        let serial_fallback = ordered.clone();
        let mut local_by_region = BTreeMap::<RegionKey, Vec<EntityKinematics>>::new();
        let mut crossings = Vec::new();
        for state in ordered {
            let Some(source) = self.regions.region_for(state.id) else {
                crossings.push(state);
                continue;
            };
            let target = RegionKey::from_position(state.position);
            if crossing_leaders.contains_key(&state.id) || target != Some(source) {
                crossings.push(state);
            } else {
                local_by_region.entry(source).or_default().push(state);
            }
        }
        let worker_count = max_workers.min(local_by_region.len());
        if worker_count <= 1 {
            return self.apply_kinematics_serial(serial_fallback);
        }

        self.transfer_tick = self
            .transfer_tick
            .checked_add(1)
            .expect("production transfer tick exhausted");
        let transfer_tick = self.transfer_tick;
        self.with_phase(|regions, phase| {
            let applied = std::sync::Mutex::new(Vec::new());
            {
                let mut batches = regions
                    .stores
                    .iter_mut()
                    .filter_map(|(&key, store)| {
                        local_by_region
                            .remove(&key)
                            .map(|states| (key, store, states))
                    })
                    .collect::<Vec<_>>();
                let mut buckets = (0..worker_count).map(|_| Vec::new()).collect::<Vec<_>>();
                for (index, batch) in batches.drain(..).enumerate() {
                    buckets[index % worker_count].push(batch);
                }
                let local = buckets.pop().expect("positive regional worker count");
                rayon::scope(|scope| {
                    for bucket in buckets {
                        let applied = &applied;
                        scope.spawn(move |_| {
                            for (key, store, states) in bucket {
                                before_region(key);
                                let accepted = apply_local_kinematics(store, states);
                                applied
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                                    .extend(accepted);
                            }
                        });
                    }
                    for (key, store, states) in local {
                        before_region(key);
                        let accepted = apply_local_kinematics(store, states);
                        applied
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .extend(accepted);
                    }
                });
            }
            let mut applied = applied
                .into_inner()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            applied.extend(apply_kinematics_in_phase(
                regions,
                phase,
                transfer_tick,
                crossings,
            ));
            applied.sort_unstable();
            applied
        })
    }

    fn apply_kinematics_serial(&mut self, states: Vec<EntityKinematics>) -> Vec<EntityId> {
        let mut states = states
            .into_iter()
            .filter(|state| state.is_finite())
            .collect::<Vec<_>>();
        if states.is_empty() {
            return Vec::new();
        }
        for state in &states {
            self.ensure_position_region(state.position);
        }
        let mut crossing_leaders = HashMap::new();
        for state in &states {
            let Some(source) = self.regions.region_for(state.id) else {
                continue;
            };
            if RegionKey::from_position(state.position) == Some(source) {
                continue;
            }
            if let Ok(group) = self.regions.vehicle_group_snapshots(source, state.id)
                && let Some(leader) = vehicle_group_leader(&group)
            {
                for member in group {
                    crossing_leaders.insert(member.id, leader);
                }
            }
        }
        states.sort_by_key(|state| {
            let leader = crossing_leaders.get(&state.id).copied().unwrap_or(state.id);
            (leader, state.id != leader)
        });
        self.transfer_tick = self
            .transfer_tick
            .checked_add(1)
            .expect("production transfer tick exhausted");
        let transfer_tick = self.transfer_tick;
        self.with_phase(|regions, phase| {
            apply_kinematics_in_phase(regions, phase, transfer_tick, states)
        })
    }

    fn authoritative_kinematics(&self, ids: Vec<EntityId>) -> Vec<EntityKinematics> {
        ids.into_iter()
            .filter_map(|id| {
                self.motion_state(id).map(|state| EntityKinematics {
                    id,
                    position: state.position,
                    rotation: state.rotation,
                    velocity: state.velocity,
                    on_ground: state.on_ground,
                })
            })
            .collect()
    }

    pub fn alive_kinematics_for_ids(&mut self, ids: &HashSet<EntityId>) -> Vec<EntityKinematics> {
        let mut states = self
            .regions
            .stores
            .values_mut()
            .flat_map(|store| store.alive_kinematics_for_ids(ids))
            .collect::<Vec<_>>();
        states.sort_unstable_by_key(|state| state.id);
        states
    }

    pub fn prepare_goal_tick_with_pathing_for_ids(
        &mut self,
        tick: u64,
        active_ids: &HashSet<EntityId>,
    ) -> RegionalPreparedGoalTick {
        let mut ids_by_region = BTreeMap::<RegionKey, HashSet<EntityId>>::new();
        for id in active_ids {
            if let Some(key) = self.regions.region_for(*id) {
                ids_by_region.entry(key).or_default().insert(*id);
            }
        }
        let captured = self.regions.capture_follow_targets(&ids_by_region);
        let batches: BTreeMap<RegionKey, PreparedGoalTick> = ids_by_region
            .into_iter()
            .map(|(key, ids)| {
                let prepared = self
                    .regions
                    .stores
                    .get_mut(&key)
                    .expect("indexed entity region")
                    .prepare_goal_tick_with_pathing_for_ids(tick, &ids);
                (key, prepared)
            })
            .collect();
        let lease_keys = batches
            .keys()
            .copied()
            .chain(captured.sources.values().map(|source| source.region))
            .collect::<BTreeSet<_>>();
        RegionalPreparedGoalTick {
            authority: self.regions.authority,
            phase: RegionPhase(0),
            leases: lease_keys
                .into_iter()
                .map(|key| {
                    (
                        key,
                        self.regions
                            .ownership
                            .lease(key)
                            .expect("production regional goal lease"),
                    )
                })
                .collect(),
            batches,
            follow_targets: captured.remote_by_region,
            follow_target_sources: captured.sources,
            expected_missing_follow_targets: captured.expected_missing_follow_targets,
            goal_inputs: captured.inputs,
        }
    }

    pub fn apply_prepared_goal_tick(
        &mut self,
        resolved: RegionalResolvedGoalTick,
    ) -> GoalTickStats {
        self.apply_prepared_goal_tick_parallel_inner(resolved, 1, &|_| {})
    }

    pub fn apply_prepared_goal_tick_parallel(
        &mut self,
        resolved: RegionalResolvedGoalTick,
        max_workers: usize,
    ) -> GoalTickStats {
        self.apply_prepared_goal_tick_parallel_inner(resolved, max_workers, &|_| {})
    }

    #[cfg(test)]
    fn apply_prepared_goal_tick_parallel_with_probe(
        &mut self,
        resolved: RegionalResolvedGoalTick,
        max_workers: usize,
        before_region: &(dyn Fn(RegionKey) + Sync),
    ) -> GoalTickStats {
        self.apply_prepared_goal_tick_parallel_inner(resolved, max_workers, before_region)
    }

    fn apply_prepared_goal_tick_parallel_inner(
        &mut self,
        mut resolved: RegionalResolvedGoalTick,
        max_workers: usize,
        before_region: &(dyn Fn(RegionKey) + Sync),
    ) -> GoalTickStats {
        if resolved.authority != self.regions.authority {
            return GoalTickStats::default();
        }
        if resolved
            .leases
            .iter()
            .any(|(key, lease)| lease.key != *key || !self.regions.ownership.validate(*lease))
        {
            return GoalTickStats::default();
        }
        if resolved
            .expected_missing_follow_targets
            .iter()
            .any(|target| self.regions.region_for(*target).is_some())
        {
            return GoalTickStats::default();
        }
        if !self
            .regions
            .follow_target_sources_match(&resolved.follow_target_sources)
        {
            return GoalTickStats::default();
        }
        if !self.regions.goal_inputs_match(&resolved.goal_inputs) {
            return GoalTickStats::default();
        }
        let worker_count = max_workers.max(1).min(resolved.batches.len().max(1));
        if worker_count == 1 {
            let mut total = GoalTickStats::default();
            let mut follow_targets = resolved.follow_targets;
            for (key, batch) in resolved.batches {
                if let Some(store) = self.regions.stores.get_mut(&key) {
                    let targets = follow_targets.remove(&key).unwrap_or_default();
                    add_goal_tick_stats(
                        &mut total,
                        store.apply_prepared_goal_tick_with_follow_targets(batch, &targets),
                    );
                }
            }
            return total;
        }

        let mut follow_targets = resolved.follow_targets;
        let mut batches = self
            .regions
            .stores
            .iter_mut()
            .filter_map(|(&key, store)| {
                let batch = resolved.batches.remove(&key)?;
                let targets = follow_targets.remove(&key).unwrap_or_default();
                Some((key, store, batch, targets))
            })
            .collect::<Vec<_>>();
        let mut buckets = (0..worker_count).map(|_| Vec::new()).collect::<Vec<_>>();
        for (index, batch) in batches.drain(..).enumerate() {
            buckets[index % worker_count].push(batch);
        }
        let local = buckets.pop().expect("positive regional worker count");
        let total = std::sync::Mutex::new(GoalTickStats::default());
        rayon::scope(|scope| {
            for bucket in buckets {
                let total = &total;
                scope.spawn(move |_| {
                    let mut worker_total = GoalTickStats::default();
                    for (key, store, batch, targets) in bucket {
                        before_region(key);
                        add_goal_tick_stats(
                            &mut worker_total,
                            store.apply_prepared_goal_tick_with_follow_targets(batch, &targets),
                        );
                    }
                    add_goal_tick_stats(
                        &mut total
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner()),
                        worker_total,
                    );
                });
            }
            let mut local_total = GoalTickStats::default();
            for (key, store, batch, targets) in local {
                before_region(key);
                add_goal_tick_stats(
                    &mut local_total,
                    store.apply_prepared_goal_tick_with_follow_targets(batch, &targets),
                );
            }
            add_goal_tick_stats(
                &mut total
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
                local_total,
            );
        });
        total
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn visit_simulation_entities(&self, visitor: impl FnMut(EntityView<'_>)) {
        self.regions.visit_simulation_entities(visitor);
    }

    pub fn visit_simulation_entities_for_ids(
        &self,
        ids: &HashSet<EntityId>,
        visitor: impl FnMut(EntityView<'_>),
    ) {
        self.regions.visit_simulation_entities_for_ids(ids, visitor);
    }

    pub fn visit_breeding_tick_entities(&self, visitor: impl FnMut(EntityView<'_>)) {
        self.regions.visit_breeding_tick_entities(visitor);
    }

    pub fn visit_sheep_entities_for_ids(
        &self,
        ids: &HashSet<EntityId>,
        visitor: impl FnMut(EntityView<'_>),
    ) {
        self.regions.visit_sheep_entities_for_ids(ids, visitor);
    }

    #[cfg(any(test, feature = "shadow-compare"))]
    #[must_use]
    pub fn shadow_comparison_stats(&self) -> &ShadowComparisonStats {
        self.regions.shadow_comparison_stats()
    }

    #[cfg(any(test, feature = "shadow-compare"))]
    pub fn compare_shadow(
        &mut self,
        tick: u64,
        stage: ShadowStage,
    ) -> Result<ShadowComparison, Box<ShadowDivergence>> {
        self.regions.compare_shadow(tick, stage)
    }

    fn ensure_position_region(&mut self, position: Vec3) -> RegionKey {
        let key = RegionKey::from_position(position).expect("finite production entity position");
        if self.regions.ownership.lease(key).is_none() {
            self.regions
                .assign_region(key, self.lane)
                .expect("inactive production region assignment");
        }
        key
    }

    fn with_entity_phase<R>(
        &mut self,
        id: EntityId,
        operation: impl FnOnce(&mut RegionalEntityStore, RegionPhase) -> R,
    ) -> Option<R> {
        self.regions.region_for(id)?;
        Some(self.with_phase(operation))
    }

    fn with_phase<R>(
        &mut self,
        operation: impl FnOnce(&mut RegionalEntityStore, RegionPhase) -> R,
    ) -> R {
        let phase = self.regions.begin_phase().expect("production region phase");
        let phase_scope = SingleLanePhase {
            regions: &mut self.regions,
            phase,
            lane: self.lane,
            finished: false,
        };
        let result = operation(phase_scope.regions, phase);
        phase_scope
            .finish()
            .expect("single production lane phase completion");
        result
    }
}

impl Default for RegionalEntityAuthority {
    fn default() -> Self {
        Self::with_next_id(0)
    }
}

fn apply_kinematics_in_phase(
    regions: &mut RegionalEntityStore,
    phase: RegionPhase,
    transfer_tick: u64,
    states: impl IntoIterator<Item = EntityKinematics>,
) -> Vec<EntityId> {
    let mut applied = Vec::new();
    let mut transferred = HashSet::new();
    for state in states {
        if transferred.contains(&state.id) {
            applied.push(state.id);
            continue;
        }
        match regions.apply_kinematics(phase, transfer_tick, state) {
            Ok(RegionalKinematicsApply::AppliedLocal) => applied.push(state.id),
            Ok(RegionalKinematicsApply::PreparedTransfer(transfer)) => {
                let group = regions.transfer_entity_ids(transfer);
                if regions
                    .decide_transfer(phase, transfer, TransferDecision::Commit)
                    .and_then(|()| regions.apply_transfer(phase, transfer))
                    .is_ok()
                {
                    transferred.extend(group);
                    applied.push(state.id);
                }
            }
            Err(_) => {}
        }
    }
    applied
}

fn apply_local_kinematics(store: &mut EntityStore, states: Vec<EntityKinematics>) -> Vec<EntityId> {
    let expected = states.clone();
    let _ = store.apply_kinematics(states);
    expected
        .into_iter()
        .filter_map(|expected| {
            store
                .motion_state(expected.id)
                .is_some_and(|actual| {
                    actual.position == expected.position
                        && actual.rotation == expected.rotation
                        && actual.velocity == expected.velocity
                        && actual.on_ground == expected.on_ground
                })
                .then_some(expected.id)
        })
        .collect()
}

impl PreparedTransfer {
    fn matches_request(
        &self,
        phase: RegionPhase,
        source: RegionLease,
        target: RegionLease,
        state: EntityKinematics,
    ) -> bool {
        self.phase == phase
            && self.source == source
            && self.target == target
            && self
                .target_snapshots
                .iter()
                .find(|snapshot| snapshot.id == state.id)
                .is_some_and(|snapshot| {
                    snapshot.position == state.position
                        && snapshot.rotation == state.rotation
                        && snapshot.velocity == state.velocity
                        && snapshot.on_ground == state.on_ground
                })
    }
}

fn entity_vehicle_reference(entity: &SpawnEntity) -> Option<EntityId> {
    entity.vehicle.and_then(|vehicle| vehicle.passenger)
}

fn snapshot_vehicle_reference(entity: &EntitySnapshot) -> Option<EntityId> {
    entity.vehicle.and_then(|vehicle| vehicle.passenger)
}

fn vehicle_group_leader(group: &[EntitySnapshot]) -> Option<EntityId> {
    let passengers = group
        .iter()
        .filter_map(snapshot_vehicle_reference)
        .collect::<HashSet<_>>();
    group
        .iter()
        .map(|snapshot| snapshot.id)
        .filter(|id| !passengers.contains(id))
        .min()
}

fn order_vehicle_group_for_removal(group: &[EntitySnapshot]) -> Vec<EntityId> {
    let by_id = group
        .iter()
        .map(|snapshot| (snapshot.id, snapshot))
        .collect::<HashMap<_, _>>();
    let passengers = group
        .iter()
        .filter_map(snapshot_vehicle_reference)
        .collect::<HashSet<_>>();
    let mut leaders = group
        .iter()
        .map(|snapshot| snapshot.id)
        .filter(|id| !passengers.contains(id))
        .collect::<Vec<_>>();
    leaders.sort_unstable();
    let mut ordered = Vec::with_capacity(group.len());
    let mut visited = HashSet::with_capacity(group.len());
    for leader in leaders {
        let mut current = Some(leader);
        while let Some(entity) = current {
            if !visited.insert(entity) {
                break;
            }
            ordered.push(entity);
            current = by_id
                .get(&entity)
                .and_then(|snapshot| snapshot_vehicle_reference(snapshot));
        }
    }
    let mut remaining = group
        .iter()
        .map(|snapshot| snapshot.id)
        .filter(|id| !visited.contains(id))
        .collect::<Vec<_>>();
    remaining.sort_unstable();
    ordered.extend(remaining);
    ordered
}

fn goal_reference(goal: &GoalState) -> Option<EntityId> {
    match goal {
        GoalState::FollowTarget { target, .. } => Some(*target),
        _ => None,
    }
}

impl Default for RegionalEntityStore {
    fn default() -> Self {
        Self::new()
    }
}

fn add_goal_tick_stats(total: &mut GoalTickStats, batch: GoalTickStats) {
    total.alive_entities += batch.alive_entities;
    total.decisions_applied += batch.decisions_applied;
    total.skipped_non_alive += batch.skipped_non_alive;
    total.missing_follow_targets += batch.missing_follow_targets;
    total.pathing_moves += batch.pathing_moves;
    total.pathing_blocked += batch.pathing_blocked;
    total.pathing_unloaded += batch.pathing_unloaded;
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    use super::{
        REGION_SIZE_CHUNKS, RegionEntityStoreError, RegionEpoch, RegionKey, RegionOwnership,
        RegionOwnershipError, RegionalEntityAuthority, RegionalEntityStore,
        RegionalShadowComparisonBatch, TransferApply, TransferDecision,
    };
    use crate::{
        AnimalBreedingState, EntityId, EntityKinematics, EntityStore, GoalState, GoalTickStats,
        PathingBudget, PathingProbe, PathingProbeResult, Rotation, ShadowStage, SheepColor,
        SpawnEntity, Vec3, VehicleKind, VehicleState,
    };
    use uuid::Uuid;

    fn parse_linux_cpu_list(value: &str) -> Option<Vec<usize>> {
        let mut cpus = std::collections::BTreeSet::new();
        for part in value.trim().split(',') {
            if part.is_empty() {
                return None;
            }
            let mut bounds = part.split('-');
            let first = bounds.next()?.parse::<usize>().ok()?;
            let last = match bounds.next() {
                Some(last) => last.parse::<usize>().ok()?,
                None => first,
            };
            if first > last || bounds.next().is_some() {
                return None;
            }
            cpus.extend(first..=last);
        }
        (!cpus.is_empty()).then(|| cpus.into_iter().collect())
    }

    #[cfg(target_os = "linux")]
    fn assert_benchmark_has_distinct_physical_cores(lanes: usize) {
        let Some(allowed) = std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|status| {
                status.lines().find_map(|line| {
                    line.strip_prefix("Cpus_allowed_list:")
                        .and_then(parse_linux_cpu_list)
                })
            })
        else {
            return;
        };
        let mut physical = std::collections::BTreeSet::new();
        for cpu in allowed {
            let topology = format!("/sys/devices/system/cpu/cpu{cpu}/topology");
            let Ok(package) = std::fs::read_to_string(format!("{topology}/physical_package_id"))
            else {
                return;
            };
            let Ok(core) = std::fs::read_to_string(format!("{topology}/core_id")) else {
                return;
            };
            physical.insert((package.trim().to_owned(), core.trim().to_owned()));
        }
        assert!(
            physical.len() >= lanes,
            "regional scaling benchmark needs {lanes} distinct physical cores; allowed affinity exposes only {} (use physical siblings such as `taskset -c 0,2`, not SMT siblings `0,1`)",
            physical.len()
        );
    }

    #[test]
    fn linux_cpu_list_parser_expands_ranges_and_singletons() {
        assert_eq!(
            parse_linux_cpu_list("0-2,5,8-9"),
            Some(vec![0, 1, 2, 5, 8, 9])
        );
        assert_eq!(parse_linux_cpu_list("2"), Some(vec![2]));
        assert_eq!(parse_linux_cpu_list("2-1"), None);
        assert_eq!(parse_linux_cpu_list("0,,2"), None);
    }

    #[derive(Default)]
    struct TestDecisionJournalState {
        commits: Vec<super::RegionalCommitDecision>,
        cleared: Vec<super::RegionPhase>,
        fail_record: bool,
        fail_record_unknown: bool,
    }

    struct TestDecisionJournal(Arc<Mutex<TestDecisionJournalState>>);

    impl super::RegionalDecisionJournal for TestDecisionJournal {
        fn record_commit(
            &mut self,
            decision: &super::RegionalCommitDecision,
        ) -> Result<(), super::RegionalDecisionJournalError> {
            let mut state = self.0.lock().expect("test journal state");
            if state.fail_record {
                return Err(super::RegionalDecisionJournalError::SAFE);
            }
            state.commits.push(decision.clone());
            if state.fail_record_unknown {
                return Err(super::RegionalDecisionJournalError::OUTCOME_UNKNOWN);
            }
            Ok(())
        }

        fn clear_commit(
            &mut self,
            phase: super::RegionPhase,
        ) -> Result<(), super::RegionalDecisionJournalError> {
            self.0
                .lock()
                .expect("test journal state")
                .cleared
                .push(phase);
            Ok(())
        }
    }

    struct BlockingClearDecisionJournal {
        state: Arc<Mutex<TestDecisionJournalState>>,
        clear_started: mpsc::SyncSender<()>,
        clear_release: mpsc::Receiver<()>,
    }

    impl super::RegionalDecisionJournal for BlockingClearDecisionJournal {
        fn record_commit(
            &mut self,
            decision: &super::RegionalCommitDecision,
        ) -> Result<(), super::RegionalDecisionJournalError> {
            self.state
                .lock()
                .expect("test journal state")
                .commits
                .push(decision.clone());
            Ok(())
        }

        fn clear_commit(
            &mut self,
            phase: super::RegionPhase,
        ) -> Result<(), super::RegionalDecisionJournalError> {
            self.clear_commits(&[phase])
        }

        fn clear_commits(
            &mut self,
            phases: &[super::RegionPhase],
        ) -> Result<(), super::RegionalDecisionJournalError> {
            self.clear_started.send(()).expect("publish clear start");
            self.clear_release.recv().expect("release journal clear");
            self.state
                .lock()
                .expect("test journal state")
                .cleared
                .extend_from_slice(phases);
            Ok(())
        }
    }

    fn cow(position: Vec3) -> SpawnEntity {
        SpawnEntity::new(4, "minecraft:cow", position)
    }

    fn movement(id: EntityId, position: Vec3) -> EntityKinematics {
        EntityKinematics {
            id,
            position,
            rotation: Rotation {
                yaw: 15.0,
                pitch: -5.0,
                head_yaw: 12.0,
            },
            velocity: Vec3::new(0.4, 0.1, -0.2),
            on_ground: false,
        }
    }

    fn spawn_shadowed_legacy(
        regions: &mut RegionalEntityStore,
        key: RegionKey,
        entity: SpawnEntity,
    ) -> EntityId {
        let mut store = EntityStore::shadowed_legacy();
        store.next_id = regions.next_id;
        let id = store.spawn(entity);
        let snapshot = store.snapshot(id).expect("legacy snapshot");
        regions.stores.insert(key, store);
        regions.locations.insert(id, key);
        regions.uuids.insert(snapshot.uuid, id);
        regions.next_id = id.0;
        id
    }

    struct WalkablePathing;

    impl PathingProbe for WalkablePathing {
        fn can_stand_at(&self, _position: Vec3) -> PathingProbeResult {
            PathingProbeResult::Walkable
        }
    }

    struct ConcurrentPathingProbe {
        entered: mpsc::Sender<EntityId>,
        release: Mutex<mpsc::Receiver<()>>,
        blocked_once: Mutex<HashSet<EntityId>>,
    }

    impl PathingProbe for ConcurrentPathingProbe {
        fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
            self.can_entity_stand_at(EntityId(0), position)
        }

        fn can_entity_stand_at(&self, entity_id: EntityId, _position: Vec3) -> PathingProbeResult {
            let first_call = self
                .blocked_once
                .lock()
                .expect("pathing call set")
                .insert(entity_id);
            if first_call {
                self.entered.send(entity_id).expect("publish entered job");
                self.release
                    .lock()
                    .expect("pathing release receiver")
                    .recv()
                    .expect("receive exact job release");
            }
            PathingProbeResult::Walkable
        }
    }

    #[test]
    fn region_key_uses_euclidean_chunk_boundaries() {
        assert_eq!(REGION_SIZE_CHUNKS, 8);
        assert_eq!(RegionKey::from_chunk(0, 0), RegionKey::new(0, 0));
        assert_eq!(RegionKey::from_chunk(7, 7), RegionKey::new(0, 0));
        assert_eq!(RegionKey::from_chunk(8, 8), RegionKey::new(1, 1));
        assert_eq!(RegionKey::from_chunk(-1, -1), RegionKey::new(-1, -1));
        assert_eq!(RegionKey::from_chunk(-8, -8), RegionKey::new(-1, -1));
        assert_eq!(RegionKey::from_chunk(-9, -9), RegionKey::new(-2, -2));

        assert_eq!(
            RegionKey::from_position(Vec3::new(127.999, 64.0, 127.999)),
            Some(RegionKey::new(0, 0))
        );
        assert_eq!(
            RegionKey::from_position(Vec3::new(128.0, 64.0, 128.0)),
            Some(RegionKey::new(1, 1))
        );
        assert_eq!(
            RegionKey::from_position(Vec3::new(-128.0, 64.0, -128.0)),
            Some(RegionKey::new(-1, -1))
        );
        assert_eq!(
            RegionKey::from_position(Vec3::new(-128.001, 64.0, -128.001)),
            Some(RegionKey::new(-2, -2))
        );
        assert_eq!(
            RegionKey::from_position(Vec3::new(f64::NAN, 64.0, 0.0)),
            None
        );
    }

    #[test]
    fn region_reassignment_is_fenced_by_phase_and_epoch() {
        let key = RegionKey::new(2, -3);
        let mut ownership = RegionOwnership::new();
        let first = ownership.assign(key, 0).expect("initial owner");
        assert_eq!(first.epoch, RegionEpoch::INITIAL);
        assert!(ownership.validate(first));

        let phase = ownership.begin_phase().expect("begin phase");
        assert_eq!(
            ownership.reassign(first, 1),
            Err(RegionOwnershipError::PhaseActive)
        );
        ownership
            .acknowledge_lane(phase, 0)
            .expect("lane 0 completes phase");
        ownership.finish_phase(phase).expect("finish phase");

        let second = ownership.reassign(first, 1).expect("next owner");
        assert_eq!(second.key, key);
        assert_eq!(second.lane, 1);
        assert_eq!(second.epoch, first.epoch.next().expect("next epoch"));
        assert!(!ownership.validate(first));
        assert!(ownership.validate(second));
        assert_eq!(
            ownership.reassign(first, 2),
            Err(RegionOwnershipError::StaleEpoch)
        );
    }

    #[test]
    fn region_phase_transitions_reject_duplicate_events() {
        let mut ownership = RegionOwnership::new();

        let first = ownership.begin_phase().expect("first begin");
        assert_eq!(
            ownership.begin_phase(),
            Err(RegionOwnershipError::PhaseActive)
        );
        ownership.finish_phase(first).expect("first finish");
        assert_eq!(
            ownership.finish_phase(first),
            Err(RegionOwnershipError::PhaseInactive)
        );

        let lease = ownership
            .assign(RegionKey::new(0, 0), 0)
            .expect("owner between phases");
        let second = ownership.begin_phase().expect("second begin");
        assert_eq!(
            ownership.finish_phase(first),
            Err(RegionOwnershipError::StalePhase)
        );
        assert_eq!(
            ownership.reassign(lease, 1),
            Err(RegionOwnershipError::PhaseActive)
        );
        assert_eq!(
            ownership.finish_phase(second),
            Err(RegionOwnershipError::PhaseIncomplete)
        );
        ownership
            .acknowledge_lane(second, 0)
            .expect("lane 0 completes second phase");
        assert_eq!(
            ownership.acknowledge_lane(second, 0),
            Err(RegionOwnershipError::DuplicateLaneCompletion)
        );
        ownership.finish_phase(second).expect("second finish");
    }

    #[test]
    fn regional_phase_waits_only_for_selected_lanes() {
        let mut ownership = RegionOwnership::new();
        ownership
            .assign(RegionKey::new(0, 0), 0)
            .expect("west owner");
        ownership
            .assign(RegionKey::new(1, 0), 1)
            .expect("east owner");

        let phase = ownership
            .begin_phase_for_lanes(std::collections::BTreeSet::from([0]))
            .expect("west-only phase");

        assert_eq!(
            ownership.validate_lane(phase, 1),
            Err(RegionOwnershipError::LaneCompleted)
        );
        ownership
            .acknowledge_lane(phase, 0)
            .expect("selected lane completes");
        ownership
            .finish_phase(phase)
            .expect("idle east lane is not part of the barrier");
    }

    #[test]
    fn region_leases_are_exposed_in_key_order() {
        let mut ownership = RegionOwnership::new();
        let east = ownership
            .assign(RegionKey::new(3, 0), 0)
            .expect("east owner");
        let west = ownership
            .assign(RegionKey::new(-2, 0), 1)
            .expect("west owner");

        assert_eq!(ownership.lease(east.key), Some(east));
        assert_eq!(ownership.lease(RegionKey::new(0, 0)), None);
        assert_eq!(ownership.leases().collect::<Vec<_>>(), vec![west, east]);
    }

    #[test]
    fn regional_entity_stores_allocate_global_ids_and_keep_state_separate() {
        let mut regions = RegionalEntityStore::new();
        let west = regions
            .assign_region(RegionKey::new(-1, 0), 0)
            .expect("west region");
        let east = regions
            .assign_region(RegionKey::new(1, 0), 0)
            .expect("east region");
        let phase = regions.begin_phase().expect("spawn phase");

        let west_id = regions
            .spawn_authoritative(phase, west, cow(Vec3::new(-0.5, 64.0, 0.5)))
            .expect("west cow");
        let east_id = regions
            .spawn_authoritative(phase, east, cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");

        assert_ne!(west_id, east_id);
        assert_eq!(regions.region_for(west_id), Some(west.key));
        assert_eq!(regions.region_for(east_id), Some(east.key));
        assert_eq!(regions.region_len(west.key), 1);
        assert_eq!(regions.region_len(east.key), 1);
        assert_eq!(
            regions.snapshot(west_id).expect("west snapshot").position,
            Vec3::new(-0.5, 64.0, 0.5)
        );
        assert_eq!(
            regions.snapshot(east_id).expect("east snapshot").position,
            Vec3::new(128.5, 64.0, 0.5)
        );
        regions
            .acknowledge_lane(phase, 0)
            .expect("lane 0 completes spawn phase");
        regions.finish_phase(phase).expect("finish spawn phase");

        let reassigned_east = regions
            .reassign_region(east, 1)
            .expect("move east region to lane 1");
        let next_phase = regions.begin_phase().expect("next phase");
        assert_eq!(
            regions.spawn_authoritative(next_phase, east, cow(Vec3::new(129.5, 64.0, 0.5)),),
            Err(RegionEntityStoreError::StaleLease)
        );
        assert!(
            regions
                .spawn_authoritative(
                    next_phase,
                    reassigned_east,
                    cow(Vec3::new(129.5, 64.0, 0.5)),
                )
                .is_ok()
        );
        regions
            .acknowledge_lane(next_phase, 0)
            .expect("lane 0 completes next phase");
        regions
            .acknowledge_lane(next_phase, 1)
            .expect("lane 1 completes next phase");
        regions.finish_phase(next_phase).expect("finish next phase");
    }

    #[test]
    fn regional_entity_stores_allow_remote_follow_but_reject_cross_region_vehicle_links() {
        let mut regions = RegionalEntityStore::new();
        let west = regions
            .assign_region(RegionKey::new(-1, 0), 0)
            .expect("west region");
        let east = regions
            .assign_region(RegionKey::new(1, 0), 0)
            .expect("east region");
        let spawn_phase = regions.begin_phase().expect("spawn phase");
        let east_id = regions
            .spawn_authoritative(spawn_phase, east, cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");
        let mut follower = cow(Vec3::new(-0.5, 64.0, 0.5));
        follower.goal = GoalState::FollowTarget {
            target: east_id,
            speed: 0.25,
        };
        assert!(
            regions
                .spawn_authoritative(spawn_phase, west, follower)
                .is_ok()
        );
        let mut boat = cow(Vec3::new(-1.5, 64.0, 0.5));
        boat.vehicle = Some(VehicleState {
            kind: VehicleKind::Boat,
            passenger: Some(east_id),
        });
        assert_eq!(
            regions.spawn_authoritative(spawn_phase, west, boat),
            Err(RegionEntityStoreError::CrossRegionReference)
        );
        assert_eq!(
            regions.spawn_authoritative(spawn_phase, west, cow(Vec3::new(0.5, 64.0, 0.5)),),
            Err(RegionEntityStoreError::WrongSpawnRegion)
        );
        assert_eq!(regions.region_len(west.key), 1);
        assert_eq!(regions.region_len(east.key), 1);
        regions
            .acknowledge_lane(spawn_phase, 0)
            .expect("lane 0 completes spawn phase");
        regions
            .finish_phase(spawn_phase)
            .expect("finish spawn phase");

        assert_eq!(
            regions.remove(spawn_phase, east, east_id),
            Err(RegionEntityStoreError::StalePhase)
        );
        assert!(regions.snapshot(east_id).is_some());
    }

    #[test]
    fn regional_entity_store_rejects_duplicate_uuid_without_consuming_id() {
        let mut regions = RegionalEntityStore::new();
        let west = regions
            .assign_region(RegionKey::new(-1, 0), 0)
            .expect("west region");
        let east = regions
            .assign_region(RegionKey::new(1, 0), 0)
            .expect("east region");
        let phase = regions.begin_phase().expect("spawn phase");
        let uuid = Uuid::from_u128(42);
        let mut east_cow = cow(Vec3::new(128.5, 64.0, 0.5));
        east_cow.uuid = Some(uuid);
        assert_eq!(
            regions
                .spawn_authoritative(phase, east, east_cow)
                .expect("east cow"),
            EntityId(1)
        );
        let mut duplicate = cow(Vec3::new(-0.5, 64.0, 0.5));
        duplicate.uuid = Some(uuid);
        assert_eq!(
            regions.spawn_authoritative(phase, west, duplicate),
            Err(RegionEntityStoreError::DuplicateUuid)
        );
        assert_eq!(
            regions
                .spawn_authoritative(phase, west, cow(Vec3::new(-0.5, 64.0, 0.5)))
                .expect("next valid cow"),
            EntityId(2)
        );
        regions
            .acknowledge_lane(phase, 0)
            .expect("lane 0 completes spawn phase");
        regions.finish_phase(phase).expect("finish spawn phase");
    }

    #[test]
    fn committed_transfer_moves_authority_once_and_replays_idempotently() {
        let mut regions = RegionalEntityStore::new();
        let west = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("west region");
        let east = regions
            .assign_region(RegionKey::new(1, 0), 1)
            .expect("east region");
        let spawn_phase = regions.begin_phase().expect("spawn phase");
        let id = regions
            .spawn_authoritative(spawn_phase, west, cow(Vec3::new(127.5, 64.0, 0.5)))
            .expect("boundary cow");
        let uuid = regions.snapshot(id).expect("source cow").uuid;
        regions
            .acknowledge_lane(spawn_phase, 0)
            .expect("west spawn complete");
        regions
            .acknowledge_lane(spawn_phase, 1)
            .expect("east spawn complete");
        regions.finish_phase(spawn_phase).expect("finish spawn");

        let phase = regions.begin_phase().expect("migration phase");
        let transfer = regions
            .prepare_transfer(
                phase,
                west,
                east,
                41,
                movement(id, Vec3::new(128.25, 64.0, 0.5)),
            )
            .expect("prepare transfer");
        assert_eq!(
            regions
                .prepare_transfer(
                    phase,
                    west,
                    east,
                    41,
                    movement(id, Vec3::new(128.25, 64.0, 0.5)),
                )
                .expect("replay prepare"),
            transfer
        );
        assert_eq!(regions.region_for(id), Some(west.key));
        assert_eq!(regions.region_len(west.key), 1);
        assert_eq!(regions.region_len(east.key), 0);
        assert_eq!(
            regions.decide_transfer(phase, transfer, TransferDecision::Commit),
            Ok(())
        );
        assert_eq!(
            regions
                .prepare_transfer(
                    phase,
                    west,
                    east,
                    41,
                    movement(id, Vec3::new(128.25, 64.0, 0.5)),
                )
                .expect("prepare after decision"),
            transfer
        );
        assert_eq!(
            regions.finish_phase(phase),
            Err(RegionEntityStoreError::Ownership(
                RegionOwnershipError::PhaseIncomplete
            ))
        );
        assert_eq!(
            regions.decide_transfer(phase, transfer, TransferDecision::Commit),
            Ok(())
        );
        assert_eq!(
            regions.decide_transfer(phase, transfer, TransferDecision::Reject),
            Err(RegionEntityStoreError::DecisionConflict)
        );
        assert_eq!(
            regions.apply_transfer(phase, transfer),
            Ok(TransferApply::Committed)
        );
        assert_eq!(
            regions
                .prepare_transfer(
                    phase,
                    west,
                    east,
                    41,
                    movement(id, Vec3::new(128.25, 64.0, 0.5)),
                )
                .expect("prepare after apply"),
            transfer
        );
        assert_eq!(
            regions.apply_transfer(phase, transfer),
            Ok(TransferApply::Committed)
        );
        let migrated = regions.snapshot(id).expect("destination cow");
        assert_eq!(migrated.id, id);
        assert_eq!(migrated.uuid, uuid);
        assert_eq!(migrated.position, Vec3::new(128.25, 64.0, 0.5));
        assert_eq!(
            migrated.rotation,
            Rotation {
                yaw: 15.0,
                pitch: -5.0,
                head_yaw: 12.0,
            }
        );
        assert_eq!(migrated.velocity, Vec3::new(0.4, 0.1, -0.2));
        assert!(!migrated.on_ground);
        assert_eq!(regions.region_for(id), Some(east.key));
        assert_eq!(regions.region_len(west.key), 0);
        assert_eq!(regions.region_len(east.key), 1);
        regions
            .acknowledge_lane(phase, 0)
            .expect("west migration complete");
        regions
            .acknowledge_lane(phase, 1)
            .expect("east migration complete");
        regions.finish_phase(phase).expect("finish migration");
        assert!(regions.transfers.is_empty());
        assert!(regions.in_flight_transfers.is_empty());
    }

    #[test]
    fn rejected_transfer_keeps_source_authority_and_replays_idempotently() {
        let mut regions = RegionalEntityStore::new();
        let west = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("west region");
        let east = regions
            .assign_region(RegionKey::new(1, 0), 1)
            .expect("east region");
        let phase = regions.begin_phase().expect("phase");
        let id = regions
            .spawn_authoritative(phase, west, cow(Vec3::new(127.5, 64.0, 0.5)))
            .expect("boundary cow");
        let transfer = regions
            .prepare_transfer(
                phase,
                west,
                east,
                7,
                movement(id, Vec3::new(128.25, 64.0, 0.5)),
            )
            .expect("prepare transfer");
        assert_eq!(
            regions.apply_transfer(phase, transfer),
            Err(RegionEntityStoreError::TransferUndecided)
        );
        assert_eq!(
            regions.remove(phase, west, id),
            Err(RegionEntityStoreError::TransferConflict)
        );
        regions
            .decide_transfer(phase, transfer, TransferDecision::Reject)
            .expect("reject transfer");
        assert_eq!(
            regions.apply_transfer(phase, transfer),
            Ok(TransferApply::Rejected)
        );
        assert_eq!(
            regions.apply_transfer(phase, transfer),
            Ok(TransferApply::Rejected)
        );
        assert_eq!(regions.region_for(id), Some(west.key));
        assert_eq!(regions.region_len(west.key), 1);
        assert_eq!(regions.region_len(east.key), 0);
        assert_eq!(
            regions.snapshot(id).expect("source cow").position,
            Vec3::new(127.5, 64.0, 0.5)
        );
        regions
            .acknowledge_lane(phase, 0)
            .expect("west rejection complete");
        regions
            .acknowledge_lane(phase, 1)
            .expect("east rejection complete");
        regions.finish_phase(phase).expect("finish rejection");
    }

    #[test]
    fn phase_boundary_rejects_undecided_transfer_and_closes_exactly_once() {
        let mut regions = RegionalEntityStore::new();
        let west = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("west region");
        let east = regions
            .assign_region(RegionKey::new(1, 0), 1)
            .expect("east region");
        let phase = regions.begin_phase().expect("phase");
        let id = regions
            .spawn_authoritative(phase, west, cow(Vec3::new(127.5, 64.0, 0.5)))
            .expect("boundary cow");
        regions
            .prepare_transfer(
                phase,
                west,
                east,
                9,
                movement(id, Vec3::new(128.25, 64.0, 0.5)),
            )
            .expect("prepare transfer");
        regions.acknowledge_lane(phase, 0).expect("west complete");
        regions.acknowledge_lane(phase, 1).expect("east complete");
        regions.finish_phase(phase).expect("implicit reject");
        assert_eq!(regions.region_for(id), Some(west.key));
        assert_eq!(
            regions.finish_phase(phase),
            Err(RegionEntityStoreError::Ownership(
                RegionOwnershipError::PhaseInactive
            ))
        );
    }

    #[test]
    fn acknowledged_lane_cannot_prepare_late_transfer() {
        let mut regions = RegionalEntityStore::new();
        let west = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("west region");
        let east = regions
            .assign_region(RegionKey::new(1, 0), 1)
            .expect("east region");
        let phase = regions.begin_phase().expect("phase");
        let id = regions
            .spawn_authoritative(phase, west, cow(Vec3::new(127.5, 64.0, 0.5)))
            .expect("boundary cow");
        regions.acknowledge_lane(phase, 0).expect("west complete");
        assert_eq!(
            regions.prepare_transfer(
                phase,
                west,
                east,
                10,
                movement(id, Vec3::new(128.25, 64.0, 0.5)),
            ),
            Err(RegionEntityStoreError::Ownership(
                RegionOwnershipError::LaneCompleted
            ))
        );
        assert_eq!(regions.region_for(id), Some(west.key));
    }

    #[test]
    fn transfer_allows_remote_follower_to_remain_in_source_region() {
        let mut regions = RegionalEntityStore::new();
        let west = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("west region");
        let east = regions
            .assign_region(RegionKey::new(1, 0), 1)
            .expect("east region");
        let phase = regions.begin_phase().expect("phase");
        let target = regions
            .spawn_authoritative(phase, west, cow(Vec3::new(127.0, 64.0, 0.5)))
            .expect("target cow");
        let mut follower = cow(Vec3::new(126.0, 64.0, 0.5));
        follower.goal = GoalState::FollowTarget {
            target,
            speed: 0.25,
        };
        let follower = regions
            .spawn_authoritative(phase, west, follower)
            .expect("source follower");
        let transfer = regions
            .prepare_transfer(
                phase,
                west,
                east,
                11,
                movement(target, Vec3::new(128.25, 64.0, 0.5)),
            )
            .expect("prepare target crossing");
        regions
            .decide_transfer(phase, transfer, TransferDecision::Commit)
            .expect("commit target crossing");
        assert_eq!(
            regions.apply_transfer(phase, transfer),
            Ok(TransferApply::Committed)
        );
        assert_eq!(regions.region_for(target), Some(east.key));
        assert_eq!(regions.region_for(follower), Some(west.key));
    }

    #[test]
    fn prepared_vehicle_group_blocks_new_vehicle_links_but_allows_remote_followers() {
        let mut regions = RegionalEntityStore::new();
        let west = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("west region");
        let east = regions
            .assign_region(RegionKey::new(1, 0), 1)
            .expect("east region");
        let phase = regions.begin_phase().expect("phase");
        let mut boat = SpawnEntity::vehicle(
            VehicleKind::Boat,
            0,
            "minecraft:oak_boat",
            Vec3::new(127.5, 64.0, 0.5),
        );
        boat.vehicle.as_mut().expect("boat state").passenger = Some(EntityId(2));
        let ids = regions
            .spawn_authoritative_batch(
                phase,
                [
                    boat,
                    cow(Vec3::new(127.75, 64.0, 0.5)),
                    cow(Vec3::new(126.5, 64.0, 0.5)),
                ],
            )
            .expect("vehicle group and follower");
        let transfer = regions
            .prepare_transfer(
                phase,
                west,
                east,
                12,
                movement(ids[0], Vec3::new(128.25, 64.0, 0.5)),
            )
            .expect("prepare vehicle group");
        let follow_passenger = GoalState::FollowTarget {
            target: ids[1],
            speed: 0.25,
        };

        assert_eq!(
            regions.set_goal(phase, ids[2], follow_passenger.clone()),
            Ok(true)
        );
        let mut new_follower = cow(Vec3::new(126.0, 64.0, 0.5));
        new_follower.goal = follow_passenger.clone();
        assert!(
            regions
                .spawn_authoritative(phase, west, new_follower)
                .is_ok()
        );
        let mut second_boat = SpawnEntity::vehicle(
            VehicleKind::Boat,
            0,
            "minecraft:oak_boat",
            Vec3::new(126.0, 64.0, 1.5),
        );
        second_boat
            .vehicle
            .as_mut()
            .expect("second boat state")
            .passenger = Some(ids[1]);
        assert_eq!(
            regions.spawn_authoritative(phase, west, second_boat),
            Err(RegionEntityStoreError::TransferConflict)
        );
        assert_eq!(
            regions.set_velocity(phase, ids[1], Vec3::new(0.1, 0.0, 0.0)),
            Err(RegionEntityStoreError::TransferConflict)
        );

        regions
            .decide_transfer(phase, transfer, TransferDecision::Reject)
            .expect("reject group");
        assert_eq!(
            regions.apply_transfer(phase, transfer),
            Ok(TransferApply::Rejected)
        );
        assert_eq!(regions.set_goal(phase, ids[2], follow_passenger), Ok(true));
        assert_eq!(
            regions.set_velocity(phase, ids[1], Vec3::new(0.1, 0.0, 0.0)),
            Ok(true)
        );
        assert_eq!(
            regions
                .snapshot(ids[0])
                .and_then(|snapshot| snapshot.vehicle)
                .and_then(|vehicle| vehicle.passenger),
            Some(ids[1])
        );
    }

    #[test]
    fn regional_kinematics_apply_local_motion_and_prepare_boundary_transfer() {
        let mut regions = RegionalEntityStore::new();
        let west = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("west region");
        let east = regions
            .assign_region(RegionKey::new(1, 0), 1)
            .expect("east region");
        let phase = regions.begin_phase().expect("phase");
        let local = regions
            .spawn_authoritative(phase, west, cow(Vec3::new(10.0, 64.0, 0.5)))
            .expect("local cow");
        let crossing = regions
            .spawn_authoritative(phase, west, cow(Vec3::new(127.5, 64.0, 0.5)))
            .expect("boundary cow");

        assert_eq!(
            regions.apply_kinematics(phase, 17, movement(local, Vec3::new(11.0, 64.0, 0.5))),
            Ok(super::RegionalKinematicsApply::AppliedLocal)
        );
        let transfer = match regions
            .apply_kinematics(phase, 17, movement(crossing, Vec3::new(128.25, 64.0, 0.5)))
            .expect("prepare crossing")
        {
            super::RegionalKinematicsApply::PreparedTransfer(transfer) => transfer,
            other => panic!("expected prepared transfer, got {other:?}"),
        };
        assert_eq!(
            regions.snapshot(local).expect("local motion").position,
            Vec3::new(11.0, 64.0, 0.5)
        );
        assert_eq!(regions.region_for(crossing), Some(west.key));
        assert_eq!(
            regions.snapshot(crossing).expect("source motion").position,
            Vec3::new(127.5, 64.0, 0.5)
        );
        assert_eq!(
            regions.apply_kinematics(phase, 17, movement(crossing, Vec3::new(127.75, 64.0, 0.5)),),
            Err(RegionEntityStoreError::TransferConflict)
        );
        regions
            .decide_transfer(phase, transfer, TransferDecision::Commit)
            .expect("commit crossing");
        assert_eq!(
            regions.apply_transfer(phase, transfer),
            Ok(TransferApply::Committed)
        );
        assert_eq!(regions.region_for(crossing), Some(east.key));
    }

    #[test]
    fn regional_global_reads_and_visitors_keep_entity_id_order() {
        let mut regions = RegionalEntityStore::new();
        let west = regions
            .assign_region(RegionKey::new(-1, 0), 0)
            .expect("west region");
        let east = regions
            .assign_region(RegionKey::new(1, 0), 0)
            .expect("east region");
        let phase = regions.begin_phase().expect("phase");
        let mut east_cow = cow(Vec3::new(128.5, 64.0, 0.5));
        east_cow.animal = Some(AnimalBreedingState::baby());
        let east_id = regions
            .spawn_authoritative(phase, east, east_cow)
            .expect("east cow");
        let mut west_sheep = SpawnEntity::new(5, "minecraft:sheep", Vec3::new(-0.5, 64.0, 0.5));
        west_sheep.animal = Some(AnimalBreedingState::adult_sheep(SheepColor::White));
        let west_id = regions
            .spawn_authoritative(phase, west, west_sheep)
            .expect("west sheep");
        let east_uuid = regions.snapshot(east_id).expect("east snapshot").uuid;

        assert_eq!(regions.len(), 2);
        assert!(!regions.is_empty());
        assert!(regions.contains(east_id));
        assert!(regions.contains_uuid(east_uuid));
        assert_eq!(
            regions
                .snapshots()
                .map(|entity| entity.id)
                .collect::<Vec<_>>(),
            vec![east_id, west_id]
        );
        assert_eq!(
            regions.motion_state(west_id).expect("west motion").position,
            Vec3::new(-0.5, 64.0, 0.5)
        );

        let mut all = Vec::new();
        regions.visit_simulation_entities(|entity| all.push(entity.id));
        assert_eq!(all, vec![east_id, west_id]);
        let mut selected = Vec::new();
        regions.visit_simulation_entities_for_ids(&HashSet::from([west_id, east_id]), |entity| {
            selected.push(entity.id)
        });
        assert_eq!(selected, vec![east_id, west_id]);
        let mut breeding = Vec::new();
        regions.visit_breeding_tick_entities(|entity| breeding.push(entity.id));
        assert_eq!(breeding, vec![east_id]);
        let mut sheep = Vec::new();
        regions.visit_sheep_entities_for_ids(&HashSet::from([east_id, west_id]), |entity| {
            sheep.push(entity.id)
        });
        assert_eq!(sheep, vec![west_id]);

        assert_eq!(
            regions.set_velocity(phase, west_id, Vec3::new(0.2, 0.0, 0.1)),
            Ok(true)
        );
        assert_eq!(
            regions
                .motion_state(west_id)
                .expect("updated west motion")
                .velocity,
            Vec3::new(0.2, 0.0, 0.1)
        );
        assert_eq!(
            regions.set_animal_state(phase, east_id, AnimalBreedingState::adult()),
            Ok(true)
        );
        assert_eq!(regions.set_goal(phase, east_id, GoalState::Idle), Ok(true));
        assert_eq!(
            regions.set_goal(
                phase,
                east_id,
                GoalState::FollowTarget {
                    target: west_id,
                    speed: 0.25,
                },
            ),
            Ok(true)
        );
        assert!(
            regions
                .damage(phase, east_id, 1.0)
                .expect("damage")
                .is_some()
        );
    }

    #[test]
    fn regional_batch_spawn_is_atomic_across_regions() {
        let mut regions = RegionalEntityStore::new();
        regions
            .assign_region(RegionKey::new(-1, 0), 0)
            .expect("west region");
        regions
            .assign_region(RegionKey::new(1, 0), 0)
            .expect("east region");
        let phase = regions.begin_phase().expect("phase");
        let ids = regions
            .spawn_authoritative_batch(
                phase,
                [
                    cow(Vec3::new(128.5, 64.0, 0.5)),
                    cow(Vec3::new(-0.5, 64.0, 0.5)),
                ],
            )
            .expect("cross-region batch");
        assert_eq!(ids, vec![EntityId(1), EntityId(2)]);
        assert_eq!(regions.region_for(ids[0]), Some(RegionKey::new(1, 0)));
        assert_eq!(regions.region_for(ids[1]), Some(RegionKey::new(-1, 0)));

        let duplicate_uuid = Uuid::from_u128(9001);
        let mut first = cow(Vec3::new(129.5, 64.0, 0.5));
        first.uuid = Some(duplicate_uuid);
        let mut second = cow(Vec3::new(-1.5, 64.0, 0.5));
        second.uuid = Some(duplicate_uuid);
        assert_eq!(
            regions.spawn_authoritative_batch(phase, [first, second]),
            Err(RegionEntityStoreError::DuplicateUuid)
        );
        assert_eq!(regions.len(), 2);
        assert_eq!(
            regions
                .spawn_authoritative_batch(phase, [cow(Vec3::new(130.5, 64.0, 0.5))])
                .expect("id after rejected batch"),
            vec![EntityId(3)]
        );
    }

    #[test]
    fn regional_restore_preserves_ids_and_rejects_cross_region_graph_atomically() {
        let mut source = RegionalEntityStore::new();
        let west = source
            .assign_region(RegionKey::new(-1, 0), 0)
            .expect("source west");
        let east = source
            .assign_region(RegionKey::new(1, 0), 0)
            .expect("source east");
        let source_phase = source.begin_phase().expect("source phase");
        let east_id = source
            .spawn_authoritative(source_phase, east, cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");
        let west_id = source
            .spawn_authoritative(source_phase, west, cow(Vec3::new(-0.5, 64.0, 0.5)))
            .expect("west cow");
        let mut boat = SpawnEntity::vehicle(
            VehicleKind::Boat,
            8,
            "minecraft:oak_boat",
            Vec3::new(-1.5, 64.0, 0.5),
        );
        boat.vehicle.as_mut().expect("boat state").passenger = Some(west_id);
        source
            .spawn_authoritative(source_phase, west, boat)
            .expect("west boat");
        let snapshots = source.snapshots().collect::<Vec<_>>();

        let mut restored = RegionalEntityStore::new();
        restored
            .assign_region(RegionKey::new(-1, 0), 0)
            .expect("restore west");
        restored
            .assign_region(RegionKey::new(1, 0), 0)
            .expect("restore east");
        let phase = restored.begin_phase().expect("restore phase");
        assert_eq!(
            restored.insert_authoritative_snapshots(phase, snapshots.clone()),
            Ok(3)
        );
        assert_eq!(restored.snapshots().collect::<Vec<_>>(), snapshots);

        let mut invalid = snapshots.clone();
        invalid[0].vehicle = Some(VehicleState {
            kind: VehicleKind::Boat,
            passenger: Some(west_id),
        });
        let mut rejected = RegionalEntityStore::new();
        rejected
            .assign_region(RegionKey::new(-1, 0), 0)
            .expect("rejected west");
        rejected
            .assign_region(RegionKey::new(1, 0), 0)
            .expect("rejected east");
        let rejected_phase = rejected.begin_phase().expect("rejected phase");
        assert_eq!(
            rejected.insert_authoritative_snapshots(rejected_phase, invalid),
            Err(RegionEntityStoreError::CrossRegionReference)
        );
        assert!(rejected.is_empty());

        let mut duplicate_vehicle = snapshots;
        let mut second_boat = duplicate_vehicle[2].clone();
        second_boat.id = EntityId(4);
        second_boat.uuid = Uuid::from_u128(9002);
        second_boat.position = Vec3::new(-2.5, 64.0, 0.5);
        duplicate_vehicle.push(second_boat);
        let mut graph_rejected = RegionalEntityStore::new();
        graph_rejected
            .assign_region(RegionKey::new(-1, 0), 0)
            .expect("graph west");
        graph_rejected
            .assign_region(RegionKey::new(1, 0), 0)
            .expect("graph east");
        let graph_phase = graph_rejected.begin_phase().expect("graph phase");
        assert_eq!(
            graph_rejected.insert_authoritative_snapshots(graph_phase, duplicate_vehicle),
            Err(RegionEntityStoreError::TargetConflict)
        );
        assert!(graph_rejected.is_empty());
        assert_eq!(east_id, EntityId(1));
    }

    #[test]
    fn batch_cannot_add_vehicle_reference_to_entity_in_transfer() {
        let mut regions = RegionalEntityStore::new();
        let west = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("west region");
        let east = regions
            .assign_region(RegionKey::new(1, 0), 1)
            .expect("east region");
        let phase = regions.begin_phase().expect("phase");
        let target = regions
            .spawn_authoritative(phase, west, cow(Vec3::new(127.5, 64.0, 0.5)))
            .expect("target cow");
        regions
            .prepare_transfer(
                phase,
                west,
                east,
                88,
                movement(target, Vec3::new(128.25, 64.0, 0.5)),
            )
            .expect("prepared target");
        let mut follower = SpawnEntity::vehicle(
            VehicleKind::Boat,
            0,
            "minecraft:oak_boat",
            Vec3::new(126.5, 64.0, 0.5),
        );
        follower.vehicle.as_mut().expect("boat state").passenger = Some(target);
        assert_eq!(
            regions.spawn_authoritative_batch(phase, [follower]),
            Err(RegionEntityStoreError::TransferConflict)
        );
        assert_eq!(regions.len(), 1);
    }

    #[test]
    fn regional_goal_batch_prepares_and_applies_across_regions() {
        let mut regions = RegionalEntityStore::new();
        let west = regions
            .assign_region(RegionKey::new(-1, 0), 0)
            .expect("west region");
        let east = regions
            .assign_region(RegionKey::new(1, 0), 1)
            .expect("east region");
        let phase = regions.begin_phase().expect("phase");

        let mut west_cow = cow(Vec3::new(-0.5, 64.0, 0.5));
        west_cow.goal = GoalState::FollowPosition {
            target: Vec3::new(-2.5, 64.0, 0.5),
            speed: 0.25,
        };
        let west_id = regions
            .spawn_authoritative(phase, west, west_cow)
            .expect("west cow");
        let mut east_cow = cow(Vec3::new(128.5, 64.0, 0.5));
        east_cow.goal = GoalState::FollowPosition {
            target: Vec3::new(130.5, 64.0, 0.5),
            speed: 0.25,
        };
        let east_id = regions
            .spawn_authoritative(phase, east, east_cow)
            .expect("east cow");

        let prepared = regions
            .prepare_goal_tick_with_pathing_for_ids(phase, 17, &HashSet::from([west_id, east_id]))
            .expect("prepare regional goals");
        let resolved = prepared.resolve(&WalkablePathing, PathingBudget::DEFAULT);
        let stats = regions
            .apply_prepared_goal_tick(phase, resolved)
            .expect("apply regional goals");

        assert_eq!(stats.alive_entities, 2);
        assert_eq!(stats.decisions_applied, 2);
        assert_eq!(stats.pathing_moves, 2);
        assert_ne!(
            regions.motion_state(west_id).expect("west motion").velocity,
            Vec3::ZERO
        );
        assert_ne!(
            regions.motion_state(east_id).expect("east motion").velocity,
            Vec3::ZERO
        );
    }

    #[test]
    fn regional_goal_parallel_batch_count_ignores_idle_regions() {
        let mut regions = RegionalEntityStore::new();
        let west = regions
            .assign_region(RegionKey::new(-1, 0), 0)
            .expect("west region");
        let east = regions
            .assign_region(RegionKey::new(1, 0), 1)
            .expect("east region");
        let phase = regions.begin_phase().expect("goal phase");
        let west_id = regions
            .spawn_authoritative(phase, west, cow(Vec3::new(-0.5, 64.0, 0.5)))
            .expect("west cow");
        let east_id = regions
            .spawn_authoritative(phase, east, cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");

        let prepared = regions
            .prepare_goal_tick_with_pathing_for_ids(phase, 31, &HashSet::from([west_id, east_id]))
            .expect("prepare regional goals");

        assert_eq!(prepared.parallel_batch_count(), 0);
    }

    #[test]
    fn regional_goal_resolve_runs_independent_regions_concurrently() {
        let mut regions = RegionalEntityStore::new();
        let west = regions
            .assign_region(RegionKey::new(-1, 0), 0)
            .expect("west region");
        let east = regions
            .assign_region(RegionKey::new(1, 0), 1)
            .expect("east region");
        let phase = regions.begin_phase().expect("goal phase");
        let mut west_cow = cow(Vec3::new(-0.5, 64.0, 0.5));
        west_cow.goal = GoalState::Wander {
            speed: 0.25,
            period_ticks: 1,
        };
        let mut east_cow = cow(Vec3::new(128.5, 64.0, 0.5));
        east_cow.goal = GoalState::Wander {
            speed: 0.25,
            period_ticks: 1,
        };
        let west_id = regions
            .spawn_authoritative(phase, west, west_cow)
            .expect("west cow");
        let east_id = regions
            .spawn_authoritative(phase, east, east_cow)
            .expect("east cow");
        let prepared = regions
            .prepare_goal_tick_with_pathing_for_ids(phase, 31, &HashSet::from([west_id, east_id]))
            .expect("prepare regional goals");
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let probe = Arc::new(ConcurrentPathingProbe {
            entered: entered_tx,
            release: Mutex::new(release_rx),
            blocked_once: Mutex::new(HashSet::new()),
        });
        let worker_probe = Arc::clone(&probe);
        let worker = std::thread::spawn(move || {
            prepared.resolve_parallel(&*worker_probe, PathingBudget::DEFAULT, 2)
        });

        let first = entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("first region entered");
        let second = entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("second region entered before either was released");
        assert_ne!(first, second);
        release_tx.send(()).expect("release first region");
        release_tx.send(()).expect("release second region");
        let resolved = worker.join().expect("regional resolve worker");
        let stats = regions
            .apply_prepared_goal_tick(phase, resolved)
            .expect("apply regional goals");
        assert_eq!(stats.decisions_applied, 2);
    }

    #[test]
    fn production_goal_apply_runs_independent_regions_concurrently() {
        let mut authority = RegionalEntityAuthority::default();
        let mut west = cow(Vec3::new(-0.5, 64.0, 0.5));
        west.goal = GoalState::Wander {
            speed: 0.25,
            period_ticks: 1,
        };
        let mut east = cow(Vec3::new(128.5, 64.0, 0.5));
        east.goal = GoalState::Wander {
            speed: 0.25,
            period_ticks: 1,
        };
        let ids = authority.spawn_authoritative_batch([west, east]);
        let resolved = authority
            .prepare_goal_tick_with_pathing_for_ids(32, &ids.into_iter().collect())
            .resolve(&WalkablePathing, PathingBudget::DEFAULT);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);

        let worker = std::thread::spawn(move || {
            authority.apply_prepared_goal_tick_parallel_with_probe(resolved, 2, &|key| {
                entered_tx.send(key).expect("goal apply probe receiver");
                release_rx
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .recv()
                    .expect("goal apply probe release");
            })
        });

        let first = entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("first goal region entered");
        let second = entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("second goal region entered before either was released");
        assert_ne!(first, second);
        release_tx.send(()).expect("release first goal region");
        release_tx.send(()).expect("release second goal region");
        let stats = worker.join().expect("regional goal apply worker");
        assert_eq!(stats.decisions_applied, 2);
    }

    #[test]
    fn regional_local_kinematics_apply_runs_independent_regions_concurrently() {
        let mut authority = RegionalEntityAuthority::default();
        let ids = authority.spawn_authoritative_batch([
            cow(Vec3::new(-0.5, 64.0, 0.5)),
            cow(Vec3::new(128.5, 64.0, 0.5)),
        ]);
        let states = [
            movement(ids[0], Vec3::new(-0.25, 64.0, 0.5)),
            movement(ids[1], Vec3::new(128.75, 64.0, 0.5)),
        ];
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);

        let worker = std::thread::spawn(move || {
            authority.apply_kinematics_parallel_with_probe(states, 2, &|key| {
                entered_tx.send(key).expect("kinematics probe receiver");
                release_rx
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .recv()
                    .expect("kinematics probe release");
            })
        });

        let first = entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("first region entered");
        let second = entered_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("second region entered before either was released");
        assert_ne!(first, second);
        release_tx.send(()).expect("release first region");
        release_tx.send(()).expect("release second region");
        assert_eq!(worker.join().expect("regional apply worker"), 2);
    }

    #[test]
    fn production_kinematics_parallelism_skips_small_batches() {
        let mut authority = RegionalEntityAuthority::default();
        let ids = authority.spawn_authoritative_batch([
            cow(Vec3::new(-0.5, 64.0, 0.5)),
            cow(Vec3::new(128.5, 64.0, 0.5)),
        ]);
        let states = [
            movement(ids[0], Vec3::new(-0.25, 64.0, 0.5)),
            movement(ids[1], Vec3::new(128.75, 64.0, 0.5)),
        ];

        assert_eq!(authority.parallel_kinematics_batch_count(&states), 0);
    }

    #[test]
    fn production_kinematics_parallelism_admits_dense_regions() {
        let entities = (0..257).map(|index| {
            if index < 128 {
                cow(Vec3::new(-0.5, 64.0, 0.5))
            } else {
                cow(Vec3::new(128.5, 64.0, 0.5))
            }
        });
        let mut authority = RegionalEntityAuthority::default();
        let ids = authority.spawn_authoritative_batch(entities);
        let states = ids
            .into_iter()
            .enumerate()
            .map(|(index, id)| {
                movement(
                    id,
                    if index < 128 {
                        Vec3::new(-0.25, 64.0, 0.5)
                    } else {
                        Vec3::new(128.75, 64.0, 0.5)
                    },
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(authority.parallel_kinematics_batch_count(&states), 2);
    }

    #[test]
    fn regional_goal_batch_rejects_a_result_from_an_old_phase() {
        let mut regions = RegionalEntityStore::new();
        let lease = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("region");
        let first_phase = regions.begin_phase().expect("first phase");
        let mut entity = cow(Vec3::new(0.5, 64.0, 0.5));
        entity.goal = GoalState::FollowPosition {
            target: Vec3::new(2.5, 64.0, 0.5),
            speed: 0.25,
        };
        let id = regions
            .spawn_authoritative(first_phase, lease, entity)
            .expect("cow");
        let prepared = regions
            .prepare_goal_tick_with_pathing_for_ids(first_phase, 18, &HashSet::from([id]))
            .expect("prepare goals");
        let resolved = prepared.resolve(&WalkablePathing, PathingBudget::DEFAULT);
        regions
            .acknowledge_lane(first_phase, 0)
            .expect("ack first phase");
        regions
            .finish_phase(first_phase)
            .expect("finish first phase");
        let second_phase = regions.begin_phase().expect("second phase");

        assert_eq!(
            regions.apply_prepared_goal_tick(second_phase, resolved),
            Err(RegionEntityStoreError::StalePhase)
        );
        assert_eq!(
            regions.motion_state(id).expect("unchanged motion").velocity,
            Vec3::ZERO
        );
    }

    #[test]
    fn regional_goal_prepare_rejects_work_after_lane_acknowledgement() {
        let mut regions = RegionalEntityStore::new();
        let lease = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("region");
        let phase = regions.begin_phase().expect("phase");
        let id = regions
            .spawn_authoritative(phase, lease, cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("cow");
        regions
            .acknowledge_lane(phase, 0)
            .expect("acknowledge lane");

        assert!(matches!(
            regions.prepare_goal_tick_with_pathing_for_ids(phase, 19, &HashSet::from([id])),
            Err(RegionEntityStoreError::Ownership(
                RegionOwnershipError::LaneCompleted
            ))
        ));
    }

    #[test]
    fn regional_goal_apply_rejects_work_after_lane_acknowledgement() {
        let mut regions = RegionalEntityStore::new();
        let lease = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("region");
        let phase = regions.begin_phase().expect("phase");
        let mut entity = cow(Vec3::new(0.5, 64.0, 0.5));
        entity.goal = GoalState::FollowPosition {
            target: Vec3::new(2.5, 64.0, 0.5),
            speed: 0.25,
        };
        let id = regions
            .spawn_authoritative(phase, lease, entity)
            .expect("cow");
        let resolved = regions
            .prepare_goal_tick_with_pathing_for_ids(phase, 20, &HashSet::from([id]))
            .expect("prepare goals")
            .resolve(&WalkablePathing, PathingBudget::DEFAULT);
        regions
            .acknowledge_lane(phase, 0)
            .expect("acknowledge lane");

        assert_eq!(
            regions.apply_prepared_goal_tick(phase, resolved),
            Err(RegionEntityStoreError::Ownership(
                RegionOwnershipError::LaneCompleted
            ))
        );
        assert_eq!(
            regions.motion_state(id).expect("unchanged motion").velocity,
            Vec3::ZERO
        );
    }

    #[test]
    fn regional_goal_apply_rejects_a_batch_from_another_store() {
        let mut source = RegionalEntityStore::new();
        let source_lease = source
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("source region");
        let source_phase = source.begin_phase().expect("source phase");
        let mut source_entity = cow(Vec3::new(0.5, 64.0, 0.5));
        source_entity.goal = GoalState::FollowPosition {
            target: Vec3::new(2.5, 64.0, 0.5),
            speed: 0.25,
        };
        let source_id = source
            .spawn_authoritative(source_phase, source_lease, source_entity)
            .expect("source cow");
        let resolved = source
            .prepare_goal_tick_with_pathing_for_ids(source_phase, 21, &HashSet::from([source_id]))
            .expect("prepare source goals")
            .resolve(&WalkablePathing, PathingBudget::DEFAULT);

        let mut target = RegionalEntityStore::new();
        let target_lease = target
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("target region");
        let target_phase = target.begin_phase().expect("target phase");
        let mut target_entity = cow(Vec3::new(0.5, 64.0, 0.5));
        target_entity.goal = GoalState::FollowPosition {
            target: Vec3::new(2.5, 64.0, 0.5),
            speed: 0.25,
        };
        let target_id = target
            .spawn_authoritative(target_phase, target_lease, target_entity)
            .expect("target cow");
        assert_eq!(source_id, target_id);

        assert_eq!(
            target.apply_prepared_goal_tick(target_phase, resolved),
            Err(RegionEntityStoreError::StaleLease)
        );
        assert_eq!(
            target
                .motion_state(target_id)
                .expect("unchanged target motion")
                .velocity,
            Vec3::ZERO
        );
    }

    #[test]
    fn regional_shadow_stats_count_one_logical_comparison() {
        let mut regions = RegionalEntityStore::new();
        regions
            .assign_region(RegionKey::new(-1, 0), 0)
            .expect("west region");
        regions
            .assign_region(RegionKey::new(1, 0), 1)
            .expect("east region");
        spawn_shadowed_legacy(
            &mut regions,
            RegionKey::new(-1, 0),
            cow(Vec3::new(-0.5, 64.0, 0.5)),
        );
        spawn_shadowed_legacy(
            &mut regions,
            RegionKey::new(1, 0),
            cow(Vec3::new(128.5, 64.0, 0.5)),
        );

        let comparison = regions
            .compare_shadow(30, ShadowStage::InputAi)
            .expect("regional shadows match");

        assert_eq!(comparison.compared_entities, 2);
        assert_eq!(comparison.compared_events, 2);
        assert_eq!(regions.shadow_comparison_stats().comparisons, 1);
        assert_eq!(regions.shadow_comparison_stats().compared_entities, 2);
        assert_eq!(regions.shadow_comparison_stats().compared_events, 2);
        assert!(regions.shadow_comparison_stats().first_divergence.is_none());
    }

    #[test]
    fn regional_shadow_comparison_preserves_first_divergence_across_calls() {
        let mut regions = RegionalEntityStore::new();
        let west_key = RegionKey::new(-1, 0);
        let east_key = RegionKey::new(1, 0);
        regions.assign_region(west_key, 0).expect("west region");
        regions.assign_region(east_key, 1).expect("east region");
        let west_id =
            spawn_shadowed_legacy(&mut regions, west_key, cow(Vec3::new(-0.5, 64.0, 0.5)));
        let east_id =
            spawn_shadowed_legacy(&mut regions, east_key, cow(Vec3::new(128.5, 64.0, 0.5)));
        regions
            .stores
            .get_mut(&east_key)
            .expect("east store")
            .perturb_shadow_position(east_id, Vec3::new(140.0, 70.0, 1.0));

        let first = regions
            .compare_shadow(31, ShadowStage::PhysicsApply)
            .expect_err("east shadow diverges first");
        assert_eq!(first.entity_id, Some(east_id));

        regions
            .stores
            .get_mut(&west_key)
            .expect("west store")
            .perturb_shadow_position(west_id, Vec3::new(-8.0, 70.0, 1.0));
        let second = regions
            .compare_shadow(32, ShadowStage::PhysicsApply)
            .expect_err("both region shadows now diverge");

        assert_eq!(second.entity_id, Some(west_id));
        assert_eq!(
            regions
                .shadow_comparison_stats()
                .first_divergence
                .as_ref()
                .and_then(|first| first.entity_id),
            Some(east_id)
        );
        assert_eq!(regions.shadow_comparison_stats().comparisons, 2);
        assert_eq!(regions.shadow_comparison_stats().compared_entities, 4);
        assert_eq!(regions.shadow_comparison_stats().compared_events, 4);
    }

    #[test]
    fn regional_shadow_comparison_reports_current_coverage_after_child_counter_saturation() {
        let mut regions = RegionalEntityStore::new();
        let key = RegionKey::new(0, 0);
        regions.assign_region(key, 0).expect("region");
        spawn_shadowed_legacy(&mut regions, key, cow(Vec3::new(0.5, 64.0, 0.5)));
        let store = regions.stores.get_mut(&key).expect("region store");
        store.shadow_stats.compared_entities = u64::MAX;
        store.shadow_stats.compared_events = u64::MAX;

        let comparison = regions
            .compare_shadow(32, ShadowStage::InputAi)
            .expect("regional shadow matches");

        assert_eq!(comparison.compared_entities, 1);
        assert_eq!(comparison.compared_events, 1);
        assert_eq!(regions.shadow_comparison_stats().compared_entities, 1);
        assert_eq!(regions.shadow_comparison_stats().compared_events, 1);
    }

    #[test]
    fn lane_local_shadow_batches_merge_without_reopening_region_stores() {
        let west_key = RegionKey::new(-1, 0);
        let east_key = RegionKey::new(1, 0);
        let mut west_store = EntityStore::shadowed_legacy();
        let west_id = west_store.spawn(cow(Vec3::new(-0.5, 64.0, 0.5)));
        west_store.perturb_shadow_position(west_id, Vec3::new(-8.0, 70.0, 1.0));
        let mut east_store = EntityStore::shadowed_legacy();
        east_store.next_id = west_id.0;
        let east_id = east_store.spawn(cow(Vec3::new(128.5, 64.0, 0.5)));
        east_store.perturb_shadow_position(east_id, Vec3::new(140.0, 70.0, 1.0));

        let mut west_batch = RegionalShadowComparisonBatch::new(33, ShadowStage::PhysicsApply);
        west_batch
            .compare_region(west_key, &mut west_store)
            .expect("west report");
        let mut east_batch = RegionalShadowComparisonBatch::new(33, ShadowStage::PhysicsApply);
        east_batch
            .compare_region(east_key, &mut east_store)
            .expect("east report");
        east_batch.merge(west_batch).expect("merge lane reports");

        let mut coordinator = RegionalEntityStore::new();
        let divergence = coordinator
            .record_shadow_comparison(east_batch.finish())
            .expect_err("both lane reports diverge");

        assert_eq!(divergence.entity_id, Some(west_id));
        assert_eq!(coordinator.shadow_comparison_stats().comparisons, 1);
        assert_eq!(coordinator.shadow_comparison_stats().compared_entities, 2);
        assert_eq!(coordinator.shadow_comparison_stats().compared_events, 4);
    }

    #[test]
    fn production_authority_rejects_non_finite_kinematics_without_panicking() {
        let mut authority = RegionalEntityAuthority::default();
        let id = authority.spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)));
        let before = authority
            .snapshot(id)
            .expect("entity before invalid motion");
        let mut invalid = movement(id, before.position);
        invalid.position.x = f64::NAN;

        assert_eq!(authority.apply_kinematics([invalid]), 0);
        assert_eq!(authority.snapshot(id), Some(before));
    }

    #[test]
    fn conditional_animal_batch_rejects_stale_snapshot_without_partial_mutation() {
        let mut authority = RegionalEntityAuthority::default();
        let ready = AnimalBreedingState {
            age_ticks: 0,
            love_ticks: 20,
            sheep_wool: None,
        };
        let mut west = cow(Vec3::new(0.5, 64.0, 0.5));
        west.animal = Some(ready);
        let mut east = cow(Vec3::new(128.5, 64.0, 0.5));
        east.animal = Some(ready);
        let ids = authority.spawn_authoritative_batch([west, east]);
        let expected = ids
            .iter()
            .map(|id| authority.snapshot(*id).expect("parent snapshot"))
            .collect::<Vec<_>>();
        assert!(authority.set_velocity(ids[0], Vec3::new(0.25, 0.0, 0.0)));

        let cooldown = AnimalBreedingState {
            age_ticks: 6_000,
            love_ticks: 0,
            sheep_wool: None,
        };
        assert!(!authority.set_animal_states_if_current([
            (expected[0].clone(), cooldown),
            (expected[1].clone(), cooldown),
        ]));
        assert_eq!(
            authority.snapshot(ids[0]).and_then(|entity| entity.animal),
            Some(ready)
        );
        assert_eq!(
            authority.snapshot(ids[1]).and_then(|entity| entity.animal),
            Some(ready)
        );
    }

    #[test]
    fn owner_coordinator_moves_physical_stores_to_lanes_and_round_trips_them() {
        let mut regions = RegionalEntityStore::new();
        let west_lease = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("west region");
        let east_lease = regions
            .assign_region(RegionKey::new(1, 0), 0)
            .expect("east region");
        let phase = regions.begin_phase().expect("spawn phase");
        let west = regions
            .spawn_authoritative(phase, west_lease, cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("west cow");
        let east = regions
            .spawn_authoritative(phase, east_lease, cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");
        regions.acknowledge_lane(phase, 0).expect("spawn lane");
        regions.finish_phase(phase).expect("spawn phase complete");

        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(regions, 2).expect("owner coordinator");
        assert_eq!(coordinator.lane_count(), 2);
        coordinator
            .set_velocities([
                (east, Vec3::new(0.2, 0.0, 0.0)),
                (west, Vec3::new(0.1, 0.0, 0.0)),
            ])
            .expect("cross-lane velocity phase");
        assert_eq!(
            coordinator
                .snapshot(west)
                .expect("west read")
                .expect("west snapshot")
                .velocity,
            Vec3::new(0.1, 0.0, 0.0)
        );
        assert_eq!(
            coordinator
                .snapshot(east)
                .expect("east read")
                .expect("east snapshot")
                .velocity,
            Vec3::new(0.2, 0.0, 0.0)
        );

        let restored = coordinator.shutdown().expect("coordinator shutdown");
        assert_eq!(restored.region_for(west), Some(RegionKey::new(0, 0)));
        assert_eq!(restored.region_for(east), Some(RegionKey::new(1, 0)));
        assert_eq!(restored.len(), 2);
        assert_eq!(
            restored.snapshot(west).expect("restored west").velocity,
            Vec3::new(0.1, 0.0, 0.0)
        );
        assert_eq!(
            restored.snapshot(east).expect("restored east").velocity,
            Vec3::new(0.2, 0.0, 0.0)
        );
    }

    #[test]
    fn owner_coordinator_excludes_idle_lanes_from_local_mutation_phase() {
        let mut regions = RegionalEntityStore::new();
        let west_lease = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("west region");
        let east_lease = regions
            .assign_region(RegionKey::new(1, 0), 0)
            .expect("east region");
        let phase = regions.begin_phase().expect("spawn phase");
        let west = regions
            .spawn_authoritative(phase, west_lease, cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("west cow");
        regions
            .spawn_authoritative(phase, east_lease, cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");
        regions.acknowledge_lane(phase, 0).expect("spawn lane");
        regions.finish_phase(phase).expect("spawn phase complete");
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(regions, 2).expect("owner coordinator");

        coordinator
            .set_velocities([(west, Vec3::new(0.1, 0.0, 0.0))])
            .expect("west-only velocity phase");

        assert_eq!(coordinator.lanes[&0].prepare_request_count(), 1);
        assert_eq!(coordinator.lanes[&0].prepare_and_commit_request_count(), 1);
        assert_eq!(coordinator.lanes[&1].prepare_request_count(), 0);
    }

    #[test]
    fn owner_standalone_local_kinematics_does_not_reread_snapshots() {
        let mut regions = RegionalEntityStore::new();
        let west_lease = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("west region");
        let east_lease = regions
            .assign_region(RegionKey::new(1, 0), 0)
            .expect("east region");
        let phase = regions.begin_phase().expect("spawn phase");
        let west = regions
            .spawn_authoritative(phase, west_lease, cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("west cow");
        let west_second = regions
            .spawn_authoritative(phase, west_lease, cow(Vec3::new(1.5, 64.0, 0.5)))
            .expect("second west cow");
        regions
            .spawn_authoritative(phase, east_lease, cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");
        regions.acknowledge_lane(phase, 0).expect("spawn lane");
        regions.finish_phase(phase).expect("spawn phase complete");
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(regions, 2).expect("owner coordinator");
        let expected = coordinator
            .snapshot(west)
            .expect("west read")
            .expect("west snapshot");
        let expected_second = coordinator
            .snapshot(west_second)
            .expect("second west read")
            .expect("second west snapshot");
        for lane in coordinator.lanes.values() {
            lane.reset_snapshot_batch_request_count();
        }
        let target = movement(west, Vec3::new(0.75, 64.0, 0.5));
        let target_second = movement(west_second, Vec3::new(1.75, 64.0, 0.5));
        let sequence_before = coordinator.commit_state.sequence_watermark();

        assert!(
            coordinator
                .apply_kinematics_if_current(
                    [(expected, target), (expected_second, target_second),]
                )
                .expect("west kinematics")
        );

        assert_eq!(
            coordinator.commit_state.sequence_watermark(),
            sequence_before + 1
        );
        assert_eq!(coordinator.lanes[&0].snapshot_batch_request_count(), 0);
        assert_eq!(coordinator.lanes[&1].snapshot_batch_request_count(), 0);
    }

    #[test]
    fn owner_plain_batch_spawn_does_not_scan_existing_lanes() {
        let mut regions = RegionalEntityStore::new();
        let west_lease = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("west region");
        let east_lease = regions
            .assign_region(RegionKey::new(1, 0), 0)
            .expect("east region");
        let phase = regions.begin_phase().expect("spawn phase");
        regions
            .spawn_authoritative(phase, west_lease, cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("west cow");
        regions
            .spawn_authoritative(phase, east_lease, cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");
        regions.acknowledge_lane(phase, 0).expect("spawn lane");
        regions.finish_phase(phase).expect("spawn phase complete");
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(regions, 2).expect("owner coordinator");
        for lane in coordinator.lanes.values() {
            lane.reset_snapshot_batch_request_count();
        }

        let spawned = coordinator
            .spawn_authoritative_batch([
                cow(Vec3::new(1.5, 64.0, 0.5)),
                cow(Vec3::new(2.5, 64.0, 0.5)),
            ])
            .expect("plain west batch");

        assert_eq!(spawned.len(), 2);
        assert_eq!(coordinator.lanes[&0].snapshot_batch_request_count(), 0);
        assert_eq!(coordinator.lanes[&1].snapshot_batch_request_count(), 0);
    }

    #[test]
    fn owner_coordinator_rolls_back_when_commit_decision_is_not_durable() {
        let mut regions = RegionalEntityStore::new();
        let lease = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("region");
        let phase = regions.begin_phase().expect("spawn phase");
        let entity = regions
            .spawn_authoritative(phase, lease, cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("cow");
        regions.acknowledge_lane(phase, 0).expect("spawn lane");
        regions.finish_phase(phase).expect("spawn phase complete");
        let journal = Arc::new(Mutex::new(TestDecisionJournalState {
            fail_record: true,
            ..TestDecisionJournalState::default()
        }));
        let mut coordinator = super::RegionalOwnerCoordinator::from_store_with_journal(
            regions,
            1,
            Box::new(TestDecisionJournal(Arc::clone(&journal))),
        )
        .expect("owner coordinator");

        assert_eq!(
            coordinator.set_velocities([(entity, Vec3::new(0.25, 0.0, 0.0))]),
            Err(super::RegionOwnerLaneError::Journal)
        );
        assert_eq!(
            coordinator
                .snapshot(entity)
                .expect("entity read")
                .expect("entity snapshot")
                .velocity,
            Vec3::ZERO
        );
        assert!(journal.lock().expect("journal state").cleared.is_empty());
        coordinator.shutdown().expect("coordinator shutdown");
    }

    #[test]
    fn owner_coordinator_fail_stops_when_journal_outcome_is_unknown() {
        let mut regions = RegionalEntityStore::new();
        let lease = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("region");
        let phase = regions.begin_phase().expect("spawn phase");
        let entity = regions
            .spawn_authoritative(phase, lease, cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("cow");
        regions.acknowledge_lane(phase, 0).expect("spawn lane");
        regions.finish_phase(phase).expect("spawn phase complete");
        let journal = Arc::new(Mutex::new(TestDecisionJournalState {
            fail_record_unknown: true,
            ..TestDecisionJournalState::default()
        }));
        let mut coordinator = super::RegionalOwnerCoordinator::from_store_with_journal(
            regions,
            1,
            Box::new(TestDecisionJournal(Arc::clone(&journal))),
        )
        .expect("owner coordinator");

        assert_eq!(
            coordinator.set_velocities([(entity, Vec3::new(0.25, 0.0, 0.0))]),
            Err(super::RegionOwnerLaneError::Journal)
        );
        assert_eq!(
            coordinator
                .snapshot(entity)
                .expect("entity read")
                .expect("entity snapshot")
                .velocity,
            Vec3::new(0.25, 0.0, 0.0)
        );
        assert_eq!(
            coordinator.set_velocities([(entity, Vec3::new(0.5, 0.0, 0.0))]),
            Err(super::RegionOwnerLaneError::Busy)
        );
        assert_eq!(journal.lock().expect("journal state").commits.len(), 1);
    }

    #[test]
    fn owner_coordinator_clears_durable_decision_only_after_saved_snapshot_checkpoint() {
        let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
        let mut coordinator = super::RegionalOwnerCoordinator::from_store_with_journal(
            RegionalEntityStore::new(),
            2,
            Box::new(TestDecisionJournal(Arc::clone(&journal))),
        )
        .expect("owner coordinator");
        let west = coordinator
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("west cow");
        let east = coordinator
            .spawn_authoritative(cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");

        coordinator
            .set_velocities([
                (west, Vec3::new(0.1, 0.0, 0.0)),
                (east, Vec3::new(0.2, 0.0, 0.0)),
            ])
            .expect("journaled velocity phase");

        let state = journal.lock().expect("journal state");
        let decision = state.commits.last().expect("recorded decision").clone();
        assert_eq!(decision.sequence_watermark(), 4);
        assert_eq!(
            decision
                .upserts()
                .iter()
                .map(|snapshot| snapshot.id)
                .collect::<Vec<_>>(),
            vec![west, east]
        );
        assert!(decision.removed().is_empty());
        assert!(
            state.cleared.is_empty(),
            "lane finalize is not a durable entity snapshot"
        );
        drop(state);

        let saved = coordinator.save_barrier().expect("owner save barrier");
        assert!(saved.journal_phases().contains(&decision.phase()));
        let saved_phases = saved.journal_phases().to_vec();
        coordinator
            .clear_recovered_commits(saved_phases.iter().copied())
            .expect("durable entity snapshot checkpoints journal");
        assert_eq!(journal.lock().expect("journal state").cleared, saved_phases);
        coordinator.shutdown().expect("coordinator shutdown");
    }

    #[test]
    fn owner_coordinator_rebalances_regions_without_losing_entities() {
        let mut regions = RegionalEntityStore::new();
        let west_lease = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("west region");
        let east_lease = regions
            .assign_region(RegionKey::new(1, 0), 0)
            .expect("east region");
        let phase = regions.begin_phase().expect("spawn phase");
        let west = regions
            .spawn_authoritative(phase, west_lease, cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("west cow");
        let east = regions
            .spawn_authoritative(phase, east_lease, cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");
        regions.acknowledge_lane(phase, 0).expect("spawn lane");
        regions.finish_phase(phase).expect("spawn phase complete");

        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(regions, 1).expect("owner coordinator");
        let stale_east = coordinator
            .ownership
            .lease(RegionKey::new(1, 0))
            .expect("east lease before scale-up");

        assert_eq!(coordinator.reconfigure_lanes(2), Ok(2));
        assert_eq!(coordinator.lane_count(), 2);
        let current_east = coordinator
            .ownership
            .lease(RegionKey::new(1, 0))
            .expect("east lease after scale-up");
        assert_ne!(current_east, stale_east);
        assert_eq!(current_east.lane, 1);
        assert_eq!(
            coordinator
                .lanes
                .get(&stale_east.lane)
                .expect("old lane")
                .snapshot(stale_east, east),
            Err(super::RegionOwnerLaneError::UnknownRegion)
        );
        assert_eq!(
            coordinator.snapshot(west).expect("west read").unwrap().id,
            west
        );
        assert_eq!(
            coordinator.snapshot(east).expect("east read").unwrap().id,
            east
        );

        assert_eq!(coordinator.reconfigure_lanes(1), Ok(1));
        assert_eq!(coordinator.lane_count(), 1);
        assert_eq!(coordinator.snapshots().expect("snapshots").len(), 2);
        let restored = coordinator.shutdown().expect("coordinator shutdown");
        assert_eq!(restored.len(), 2);
    }

    #[test]
    fn owner_coordinator_aborts_ready_lane_when_peer_rejects_prepare() {
        let mut regions = RegionalEntityStore::new();
        let west_lease = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("west region");
        let east_lease = regions
            .assign_region(RegionKey::new(1, 0), 0)
            .expect("east region");
        let phase = regions.begin_phase().expect("spawn phase");
        let west = regions
            .spawn_authoritative(phase, west_lease, cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("west cow");
        let east = regions
            .spawn_authoritative(phase, east_lease, cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");
        regions.acknowledge_lane(phase, 0).expect("spawn lane");
        regions.finish_phase(phase).expect("spawn phase complete");
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(regions, 2).expect("owner coordinator");
        let east_key = RegionKey::new(1, 0);
        let current = coordinator
            .ownership
            .lease(east_key)
            .expect("east coordinator lease");
        coordinator
            .ownership
            .reassign(current, current.lane)
            .expect("coordinator-only stale epoch");

        assert_eq!(
            coordinator.set_velocities([
                (west, Vec3::new(0.1, 0.0, 0.0)),
                (east, Vec3::new(0.2, 0.0, 0.0)),
            ]),
            Err(super::RegionOwnerLaneError::StaleLease)
        );
        let restored = coordinator.shutdown().expect("coordinator shutdown");
        assert_eq!(restored.snapshot(west).expect("west").velocity, Vec3::ZERO);
        assert_eq!(restored.snapshot(east).expect("east").velocity, Vec3::ZERO);
    }

    #[test]
    fn owner_coordinator_spawns_into_an_empty_world_and_balances_new_regions() {
        let empty = RegionalEntityStore::new();
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(empty, 2).expect("empty owner coordinator");
        assert_eq!(coordinator.lane_count(), 2);

        let west = coordinator
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("west cow");
        let east = coordinator
            .spawn_authoritative(cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");

        assert_eq!(west, EntityId(1));
        assert_eq!(east, EntityId(2));
        assert_eq!(
            coordinator
                .snapshot(west)
                .expect("west read")
                .expect("west snapshot")
                .position,
            Vec3::new(0.5, 64.0, 0.5)
        );
        assert_eq!(
            coordinator
                .snapshot(east)
                .expect("east read")
                .expect("east snapshot")
                .position,
            Vec3::new(128.5, 64.0, 0.5)
        );
        assert_ne!(
            coordinator
                .ownership
                .lease(RegionKey::new(0, 0))
                .expect("west lease")
                .lane,
            coordinator
                .ownership
                .lease(RegionKey::new(1, 0))
                .expect("east lease")
                .lane
        );
        assert_eq!(
            coordinator
                .snapshots()
                .expect("coordinator snapshot batch")
                .into_iter()
                .map(|snapshot| snapshot.id)
                .collect::<Vec<_>>(),
            vec![west, east]
        );

        let restored = coordinator.shutdown().expect("coordinator shutdown");
        assert_eq!(restored.len(), 2);
        assert_eq!(restored.region_for(west), Some(RegionKey::new(0, 0)));
        assert_eq!(restored.region_for(east), Some(RegionKey::new(1, 0)));
    }

    #[test]
    fn owner_coordinator_removes_spawned_entity_without_leaving_global_indexes() {
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
                .expect("empty owner coordinator");
        let entity = coordinator
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("spawn cow");

        let removed = coordinator
            .remove(entity)
            .expect("remove cow")
            .expect("removed snapshot");

        assert_eq!(removed.id, entity);
        assert_eq!(
            coordinator.snapshot(entity).expect("read after remove"),
            None
        );
        let restored = coordinator.shutdown().expect("coordinator shutdown");
        assert!(!restored.contains(entity));
        assert!(!restored.contains_uuid(removed.uuid));
    }

    #[test]
    fn owner_coordinator_rejects_cross_lane_animal_cas_when_one_parent_is_stale() {
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
                .expect("empty owner coordinator");
        let ready = AnimalBreedingState {
            age_ticks: 0,
            love_ticks: 20,
            sheep_wool: None,
        };
        let mut west = cow(Vec3::new(0.5, 64.0, 0.5));
        west.animal = Some(ready);
        let mut east = cow(Vec3::new(128.5, 64.0, 0.5));
        east.animal = Some(ready);
        let west = coordinator.spawn_authoritative(west).expect("west cow");
        let east = coordinator.spawn_authoritative(east).expect("east cow");
        let expected_west = coordinator
            .snapshot(west)
            .expect("west read")
            .expect("west snapshot");
        let expected_east = coordinator
            .snapshot(east)
            .expect("east read")
            .expect("east snapshot");
        coordinator
            .set_velocities([(east, Vec3::new(0.25, 0.0, 0.0))])
            .expect("make east stale");
        let cooldown = AnimalBreedingState {
            age_ticks: 6_000,
            love_ticks: 0,
            sheep_wool: None,
        };

        assert!(
            !coordinator
                .set_animal_states_if_current([
                    (expected_west, cooldown),
                    (expected_east, cooldown),
                ])
                .expect("rejected CAS")
        );
        assert_eq!(
            coordinator
                .snapshot(west)
                .expect("west read")
                .expect("west snapshot")
                .animal,
            Some(ready)
        );
        assert_eq!(
            coordinator
                .snapshot(east)
                .expect("east read")
                .expect("east snapshot")
                .animal,
            Some(ready)
        );
        let current_west = coordinator
            .snapshot(west)
            .expect("west read")
            .expect("current west snapshot");
        let current_east = coordinator
            .snapshot(east)
            .expect("east read")
            .expect("current east snapshot");
        assert!(
            coordinator
                .set_animal_states_if_current([(current_west, cooldown), (current_east, cooldown),])
                .expect("accepted CAS")
        );
        assert_eq!(
            coordinator
                .snapshot(west)
                .expect("west read")
                .expect("west snapshot")
                .animal,
            Some(cooldown)
        );
        assert_eq!(
            coordinator
                .snapshot(east)
                .expect("east read")
                .expect("east snapshot")
                .animal,
            Some(cooldown)
        );
    }

    #[test]
    fn owner_coordinator_rejects_cross_lane_kinematics_when_one_input_is_stale() {
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
                .expect("empty owner coordinator");
        let west = coordinator
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("west cow");
        let east = coordinator
            .spawn_authoritative(cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");
        let expected_west = coordinator
            .snapshot(west)
            .expect("west read")
            .expect("west snapshot");
        let expected_east = coordinator
            .snapshot(east)
            .expect("east read")
            .expect("east snapshot");
        coordinator
            .set_velocities([(east, Vec3::new(0.25, 0.0, 0.0))])
            .expect("make east stale");

        assert!(
            !coordinator
                .apply_kinematics_if_current([
                    (expected_west, movement(west, Vec3::new(1.5, 64.0, 0.5))),
                    (expected_east, movement(east, Vec3::new(129.5, 64.0, 0.5)),),
                ])
                .expect("rejected kinematics")
        );
        assert_eq!(
            coordinator
                .snapshot(west)
                .expect("west read")
                .expect("west snapshot")
                .position,
            Vec3::new(0.5, 64.0, 0.5)
        );
        assert_eq!(
            coordinator
                .snapshot(east)
                .expect("east read")
                .expect("east snapshot")
                .velocity,
            Vec3::new(0.25, 0.0, 0.0)
        );

        let current_west = coordinator
            .snapshot(west)
            .expect("west read")
            .expect("current west snapshot");
        let current_east = coordinator
            .snapshot(east)
            .expect("east read")
            .expect("current east snapshot");
        assert!(
            coordinator
                .apply_kinematics_if_current([
                    (current_west, movement(west, Vec3::new(1.5, 64.0, 0.5))),
                    (current_east, movement(east, Vec3::new(129.5, 64.0, 0.5)),),
                ])
                .expect("accepted kinematics")
        );
        assert_eq!(
            coordinator
                .snapshot(west)
                .expect("west read")
                .expect("west snapshot")
                .position,
            Vec3::new(1.5, 64.0, 0.5)
        );
        assert_eq!(
            coordinator
                .snapshot(east)
                .expect("east read")
                .expect("east snapshot")
                .position,
            Vec3::new(129.5, 64.0, 0.5)
        );
    }

    #[test]
    fn owner_coordinator_moves_standalone_entity_across_region_owners() {
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
                .expect("empty owner coordinator");
        let entity = coordinator
            .spawn_authoritative(cow(Vec3::new(127.5, 64.0, 0.5)))
            .expect("boundary cow");
        let expected = coordinator
            .snapshot(entity)
            .expect("cow read")
            .expect("cow snapshot");
        let target = movement(entity, Vec3::new(128.5, 64.0, 0.5));
        coordinator
            .set_velocities([(entity, Vec3::new(0.25, 0.0, 0.0))])
            .expect("make source stale");

        assert!(
            !coordinator
                .apply_kinematics_if_current([(expected, target)])
                .expect("reject stale boundary kinematics")
        );
        assert_eq!(
            coordinator
                .snapshot(entity)
                .expect("cow read")
                .expect("source cow")
                .position,
            Vec3::new(127.5, 64.0, 0.5)
        );
        assert_eq!(coordinator.locations[&entity], RegionKey::new(0, 0));
        let expected = coordinator
            .snapshot(entity)
            .expect("cow read")
            .expect("fresh cow snapshot");

        assert!(
            coordinator
                .apply_kinematics_if_current([(expected, target)])
                .expect("boundary kinematics")
        );
        assert_eq!(
            coordinator
                .snapshot(entity)
                .expect("cow read")
                .expect("migrated cow")
                .position,
            target.position
        );
        assert_eq!(
            coordinator
                .ownership
                .lease(RegionKey::new(0, 0))
                .expect("source lease")
                .lane,
            0
        );
        assert_eq!(
            coordinator
                .ownership
                .lease(RegionKey::new(1, 0))
                .expect("target lease")
                .lane,
            1
        );

        let restored = coordinator.shutdown().expect("coordinator shutdown");
        assert_eq!(restored.region_for(entity), Some(RegionKey::new(1, 0)));
        assert_eq!(
            restored.snapshot(entity).expect("restored cow").position,
            target.position
        );
    }

    #[test]
    fn owner_coordinator_damage_uses_snapshot_cas_and_reports_lethal_result() {
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
                .expect("empty owner coordinator");
        let entity = coordinator
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("cow");
        let expected = coordinator
            .snapshot(entity)
            .expect("cow read")
            .expect("cow snapshot");

        let damaged = coordinator
            .damage_if_current(expected.clone(), 5.0)
            .expect("damage")
            .expect("accepted damage");
        assert_eq!(damaged.snapshot.health, 15.0);
        assert!(!damaged.killed);
        assert!(
            coordinator
                .damage_if_current(expected, 5.0)
                .expect("stale damage")
                .is_none()
        );
        assert_eq!(
            coordinator
                .snapshot(entity)
                .expect("cow read")
                .expect("cow snapshot")
                .health,
            15.0
        );

        let current = coordinator
            .snapshot(entity)
            .expect("cow read")
            .expect("current cow snapshot");
        let killed = coordinator
            .damage_if_current(current, 20.0)
            .expect("lethal damage")
            .expect("accepted lethal damage");
        assert!(killed.killed);
        assert_eq!(killed.snapshot.health, 0.0);
        assert_eq!(
            killed.snapshot.lifecycle,
            crate::EntityLifecycle::Despawning
        );
    }

    #[test]
    fn owner_coordinator_goal_apply_rejects_stale_lane_then_commits_fresh_batches() {
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
                .expect("empty owner coordinator");
        let mut west = cow(Vec3::new(0.5, 64.0, 0.5));
        west.goal = GoalState::Wander {
            speed: 0.2,
            period_ticks: 20,
        };
        let mut east = cow(Vec3::new(128.5, 64.0, 0.5));
        east.goal = west.goal.clone();
        let west = coordinator.spawn_authoritative(west).expect("west cow");
        let east = coordinator.spawn_authoritative(east).expect("east cow");
        let active = HashSet::from([west, east]);
        let prepared = coordinator
            .prepare_goal_tick_with_pathing_for_ids(23, &active)
            .expect("prepare goals");
        coordinator
            .set_velocities([(east, Vec3::new(0.25, 0.0, 0.0))])
            .expect("make east stale");

        let rejected = coordinator
            .apply_prepared_goal_tick(prepared.resolve(&WalkablePathing, PathingBudget::DEFAULT))
            .expect("reject stale goals");
        assert_eq!(rejected, GoalTickStats::default());
        assert_eq!(
            coordinator
                .snapshot(west)
                .expect("west read")
                .expect("west snapshot")
                .velocity,
            Vec3::ZERO
        );
        assert_eq!(
            coordinator
                .snapshot(east)
                .expect("east read")
                .expect("east snapshot")
                .velocity,
            Vec3::new(0.25, 0.0, 0.0)
        );

        let prepared = coordinator
            .prepare_goal_tick_with_pathing_for_ids(23, &active)
            .expect("prepare fresh goals");
        let applied = coordinator
            .apply_prepared_goal_tick(prepared.resolve(&WalkablePathing, PathingBudget::DEFAULT))
            .expect("apply fresh goals");
        assert_eq!(applied.decisions_applied, 2);
        assert_ne!(
            coordinator
                .snapshot(west)
                .expect("west read")
                .expect("west snapshot")
                .velocity,
            Vec3::ZERO
        );
        assert_ne!(
            coordinator
                .snapshot(east)
                .expect("east read")
                .expect("east snapshot")
                .velocity,
            Vec3::new(0.25, 0.0, 0.0)
        );
    }

    #[test]
    fn owner_goal_apply_returns_sorted_kinematics_without_post_apply_snapshots() {
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
                .expect("owner coordinator");
        let mut west = cow(Vec3::new(0.5, 64.0, 0.5));
        west.goal = GoalState::Wander {
            speed: 0.2,
            period_ticks: 20,
        };
        let mut east = cow(Vec3::new(128.5, 64.0, 0.5));
        east.goal = west.goal.clone();
        let west = coordinator.spawn_authoritative(west).expect("west cow");
        let east = coordinator.spawn_authoritative(east).expect("east cow");
        let active = HashSet::from([east, west]);
        let resolved = coordinator
            .prepare_goal_tick_with_pathing_for_ids(23, &active)
            .expect("prepare goals")
            .resolve(&WalkablePathing, PathingBudget::DEFAULT);
        for lane in coordinator.lanes.values() {
            lane.reset_snapshot_batch_request_count();
        }

        let (stats, projection) = coordinator
            .apply_prepared_goal_tick_and_kinematics_for_ids(resolved, &active, true)
            .expect("apply goals and project kinematics")
            .expect("fresh goals apply");

        assert_eq!(stats.decisions_applied, 2);
        assert_eq!(
            projection.iter().map(|state| state.id).collect::<Vec<_>>(),
            vec![west, east]
        );
        assert!(projection.iter().all(|state| state.velocity != Vec3::ZERO));
        assert!(
            coordinator
                .lanes
                .values()
                .all(|lane| lane.snapshot_batch_request_count() == 0),
            "typed post-apply projection must not materialize full snapshots"
        );

        let stale = coordinator
            .prepare_goal_tick_with_pathing_for_ids(24, &active)
            .expect("prepare stale goals")
            .resolve(&WalkablePathing, PathingBudget::DEFAULT);
        let current_velocity = Vec3::new(0.125, 0.25, 0.375);
        coordinator
            .set_velocities([(west, current_velocity)])
            .expect("make prepared goals stale");
        for lane in coordinator.lanes.values() {
            lane.reset_snapshot_batch_request_count();
        }
        let rejected = coordinator
            .apply_prepared_goal_tick_and_kinematics_for_ids(stale, &active, true)
            .expect("reject stale goals");

        assert!(rejected.is_none());
        assert_eq!(
            coordinator
                .snapshot(west)
                .expect("west read")
                .expect("west snapshot")
                .velocity,
            current_velocity
        );
        assert!(
            coordinator
                .lanes
                .values()
                .all(|lane| lane.snapshot_batch_request_count() == 0),
            "stale typed projection must not materialize full snapshots"
        );
    }

    #[test]
    fn owner_goal_tick_does_not_snapshot_inactive_lanes() {
        let mut regions = RegionalEntityStore::new();
        let follower_lease = regions
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("follower region");
        let target_lease = regions
            .assign_region(RegionKey::new(1, 0), 1)
            .expect("target region");
        let unrelated_lease = regions
            .assign_region(RegionKey::new(2, 0), 2)
            .expect("unrelated region");
        let phase = regions.begin_phase().expect("spawn phase");
        let follower = regions
            .spawn_authoritative(phase, follower_lease, cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("follower");
        let target = regions
            .spawn_authoritative(phase, target_lease, cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("target");
        regions
            .spawn_authoritative(phase, unrelated_lease, cow(Vec3::new(256.5, 64.0, 0.5)))
            .expect("unrelated cow");
        regions.acknowledge_lane(phase, 0).expect("follower lane");
        regions.acknowledge_lane(phase, 1).expect("target lane");
        regions.acknowledge_lane(phase, 2).expect("unrelated lane");
        regions.finish_phase(phase).expect("spawn complete");
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(regions, 3).expect("owner coordinator");
        assert!(
            coordinator
                .set_goal(follower, GoalState::FollowTarget { target, speed: 0.2 },)
                .expect("set follower goal")
        );
        let cached_follower = coordinator
            .snapshots_for_ids(&HashSet::from([follower]))
            .expect("cache follower input");
        for lane in coordinator.lanes.values() {
            lane.reset_snapshot_batch_request_count();
        }

        let resolved = coordinator
            .prepare_goal_tick_with_pathing_for_ids_from_snapshots(
                23,
                &HashSet::from([follower]),
                Some(&cached_follower),
            )
            .expect("prepare follower goal")
            .resolve(&WalkablePathing, PathingBudget::DEFAULT);
        assert_eq!(coordinator.lanes[&0].snapshot_batch_request_count(), 0);
        assert_eq!(coordinator.lanes[&1].snapshot_batch_request_count(), 1);
        assert_eq!(coordinator.lanes[&2].snapshot_batch_request_count(), 0);

        let stats = coordinator
            .apply_prepared_goal_tick(resolved)
            .expect("apply follower goal");

        assert_eq!(stats.decisions_applied, 1);
        assert_eq!(coordinator.lanes[&0].snapshot_batch_request_count(), 0);
        assert_eq!(coordinator.lanes[&1].snapshot_batch_request_count(), 2);
        assert_eq!(coordinator.lanes[&2].snapshot_batch_request_count(), 0);
    }

    #[test]
    fn owner_goal_apply_lets_lane_reject_stale_local_input_without_resnapshot() {
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
                .expect("owner coordinator");
        let follower = coordinator
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("follower");
        assert!(
            coordinator
                .set_goal(
                    follower,
                    GoalState::FollowPosition {
                        target: Vec3::new(4.5, 64.0, 0.5),
                        speed: 0.2,
                    },
                )
                .expect("set follower goal")
        );
        let resolved = coordinator
            .prepare_goal_tick_with_pathing_for_ids(23, &HashSet::from([follower]))
            .expect("prepare follower goal")
            .resolve(&WalkablePathing, PathingBudget::DEFAULT);
        assert!(
            coordinator
                .set_position(follower, Vec3::new(1.5, 64.0, 0.5))
                .expect("move follower")
        );
        coordinator.lanes[&0].reset_snapshot_batch_request_count();

        let stats = coordinator
            .apply_prepared_goal_tick(resolved)
            .expect("reject stale follower goal");

        assert_eq!(stats, GoalTickStats::default());
        assert_eq!(coordinator.lanes[&0].snapshot_batch_request_count(), 0);
        assert_eq!(
            coordinator
                .snapshot(follower)
                .expect("follower read")
                .expect("follower snapshot")
                .position,
            Vec3::new(1.5, 64.0, 0.5)
        );
    }

    #[test]
    fn owner_goal_tick_rejects_a_missing_target_replaced_before_apply() {
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
                .expect("empty owner coordinator");
        let target = coordinator
            .spawn_authoritative(cow(Vec3::new(1.5, 64.0, 0.5)))
            .expect("target");
        let follower = coordinator
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("follower");
        coordinator
            .spawn_authoritative(cow(Vec3::new(3.5, 64.0, 0.5)))
            .expect("unrelated cow");
        assert!(
            coordinator
                .set_goal(follower, GoalState::FollowTarget { target, speed: 0.2 },)
                .expect("set follower goal")
        );
        let removed = coordinator
            .remove(target)
            .expect("remove target")
            .expect("target snapshot");
        let follower_before = coordinator
            .snapshot(follower)
            .expect("follower read")
            .expect("follower snapshot");
        let resolved = coordinator
            .prepare_goal_tick_with_pathing_for_ids(23, &HashSet::from([follower]))
            .expect("prepare missing target goal")
            .resolve(&WalkablePathing, PathingBudget::DEFAULT);
        let mut replacement = removed;
        replacement.uuid = Uuid::from_u128(23);
        replacement.position = Vec3::new(2.5, 64.0, 0.5);
        assert_eq!(
            coordinator
                .insert_authoritative_snapshots_batch([replacement])
                .expect("restore replacement"),
            1
        );

        let stats = coordinator
            .apply_prepared_goal_tick(resolved)
            .expect("reject replacement target");

        assert_eq!(stats, GoalTickStats::default());
        assert_eq!(
            coordinator
                .snapshot(follower)
                .expect("follower after apply"),
            Some(follower_before)
        );
    }

    #[test]
    fn owner_save_barrier_captures_one_finalized_sequence_and_stable_snapshots() {
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
                .expect("empty owner coordinator");
        let west = coordinator
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("west cow");
        let east = coordinator
            .spawn_authoritative(cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");
        coordinator
            .set_velocities([
                (west, Vec3::new(0.1, 0.0, 0.0)),
                (east, Vec3::new(0.2, 0.0, 0.0)),
            ])
            .expect("finalized motion");

        let saved = coordinator.save_barrier().expect("save barrier");
        assert!(saved.sequence_watermark() > 0);
        assert_eq!(
            saved
                .snapshots()
                .iter()
                .map(|snapshot| snapshot.id)
                .collect::<Vec<_>>(),
            vec![west, east]
        );
        assert_eq!(saved.snapshots()[0].velocity, Vec3::new(0.1, 0.0, 0.0));
        assert_eq!(saved.snapshots()[1].velocity, Vec3::new(0.2, 0.0, 0.0));

        coordinator
            .set_velocities([(west, Vec3::new(0.3, 0.0, 0.0))])
            .expect("later motion");
        assert_eq!(saved.snapshots()[0].velocity, Vec3::new(0.1, 0.0, 0.0));
        let newer = coordinator.save_barrier().expect("newer save barrier");
        assert!(newer.sequence_watermark() > saved.sequence_watermark());
        assert_eq!(newer.snapshots()[0].velocity, Vec3::new(0.3, 0.0, 0.0));
    }

    #[test]
    fn checkpoint_does_not_acknowledge_a_durable_mutation_appended_after_its_snapshot() {
        let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
        let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
            RegionalEntityStore::new(),
            1,
            Box::new(TestDecisionJournal(Arc::clone(&journal))),
        )
        .expect("owner runtime");
        let handle = runtime.handle();
        let mut spawn = cow(Vec3::new(0.5, 64.0, 0.5));
        spawn.animal = Some(AnimalBreedingState::adult());
        let entity = handle
            .spawn_authoritative(spawn)
            .expect("durable cow spawn");
        let initial = handle.save_barrier().expect("initial save barrier");
        handle
            .clear_recovered_commits(initial.journal_phases().iter().copied())
            .expect("clear initial journal phase");
        journal.lock().expect("journal state").cleared.clear();
        let mut later_snapshot = handle
            .snapshot(entity)
            .expect("current entity snapshot")
            .expect("spawned cow");
        later_snapshot.goal = GoalState::FollowPosition {
            target: Vec3::new(4.5, 64.0, 0.5),
            speed: 0.25,
        };

        let (snapshot_entered, snapshot_entered_rx) = mpsc::sync_channel(0);
        let (snapshot_release, snapshot_release_rx) = mpsc::sync_channel(0);
        handle
            .commit_state
            .pause_before_save_barrier_phase_snapshot(snapshot_entered, snapshot_release_rx);

        let checkpoint_handle = handle.clone();
        let (checkpoint_done, checkpoint_done_rx) = mpsc::channel();
        let checkpoint_thread = std::thread::spawn(move || {
            checkpoint_done
                .send(checkpoint_handle.save_barrier())
                .expect("publish checkpoint result");
        });
        snapshot_entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("checkpoint captured entity snapshot before journal phases");

        let appended_phase = super::RegionPhase(u64::MAX - 1);
        handle
            .commit_state
            .record_commit(
                &super::RegionalCommitDecision::from_parts(
                    appended_phase,
                    u64::MAX,
                    vec![later_snapshot],
                    Vec::new(),
                )
                .expect("valid later decision"),
            )
            .expect("append later durable decision");

        snapshot_release
            .send(())
            .expect("release checkpoint phase snapshot");
        let checkpoint = checkpoint_done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("checkpoint result")
            .expect("checkpoint succeeds");
        checkpoint_thread.join().expect("join checkpoint");

        assert_eq!(
            checkpoint.snapshots()[0].goal,
            GoalState::Idle,
            "the checkpoint snapshot predates the concurrent durable decision"
        );
        assert!(
            !checkpoint.journal_phases().contains(&appended_phase),
            "a checkpoint must not acknowledge a journal decision absent from its entity snapshot"
        );
        handle
            .clear_recovered_commits(checkpoint.journal_phases().iter().copied())
            .expect("acknowledge checkpoint journal phases");
        assert!(
            !journal
                .lock()
                .expect("journal state")
                .cleared
                .contains(&appended_phase),
            "compaction must retain the mutation for a later checkpoint"
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn owner_restore_batch_preserves_local_vehicle_graph_and_rejects_cross_region_graph() {
        let mut source = RegionalEntityStore::new();
        let west_lease = source
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("source region");
        let phase = source.begin_phase().expect("source phase");
        let passenger = source
            .spawn_authoritative(phase, west_lease, cow(Vec3::new(1.5, 64.0, 0.5)))
            .expect("passenger");
        let mut boat = SpawnEntity::vehicle(
            VehicleKind::Boat,
            8,
            "minecraft:oak_boat",
            Vec3::new(0.5, 64.0, 0.5),
        );
        boat.vehicle.as_mut().expect("boat state").passenger = Some(passenger);
        source
            .spawn_authoritative(phase, west_lease, boat)
            .expect("boat");
        let snapshots = source.snapshots().collect::<Vec<_>>();

        let mut restored =
            super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
                .expect("restore coordinator");
        assert_eq!(
            restored.insert_authoritative_snapshots_batch(snapshots.clone()),
            Ok(2)
        );
        assert_eq!(
            restored
                .save_barrier()
                .expect("restored barrier")
                .into_snapshots(),
            snapshots
        );

        let mut invalid = snapshots;
        invalid
            .iter_mut()
            .find(|snapshot| snapshot.vehicle.is_some())
            .expect("boat snapshot")
            .position = Vec3::new(128.5, 64.0, 0.5);
        let mut rejected =
            super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
                .expect("reject coordinator");
        assert_eq!(
            rejected.insert_authoritative_snapshots_batch(invalid),
            Err(super::RegionOwnerLaneError::InvalidMutation)
        );
        assert!(
            rejected
                .save_barrier()
                .expect("empty barrier")
                .snapshots()
                .is_empty()
        );
    }

    #[test]
    fn owner_coordinator_moves_vehicle_group_across_lanes_with_leader_delta() {
        let mut source = RegionalEntityStore::new();
        let source_lease = source
            .assign_region(RegionKey::new(0, 0), 0)
            .expect("source region");
        let phase = source.begin_phase().expect("source phase");
        let passenger = source
            .spawn_authoritative(phase, source_lease, cow(Vec3::new(127.0, 64.0, 0.5)))
            .expect("passenger");
        let mut boat = SpawnEntity::vehicle(
            VehicleKind::Boat,
            8,
            "minecraft:oak_boat",
            Vec3::new(127.5, 64.0, 0.5),
        );
        boat.vehicle.as_mut().expect("boat state").passenger = Some(passenger);
        let boat = source
            .spawn_authoritative(phase, source_lease, boat)
            .expect("boat");
        let snapshots = source.snapshots().collect::<Vec<_>>();
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
                .expect("owner coordinator");
        coordinator
            .insert_authoritative_snapshots_batch(snapshots)
            .expect("restore vehicle group");
        let expected_boat = coordinator
            .snapshot(boat)
            .expect("boat read")
            .expect("boat snapshot");
        let expected_passenger = coordinator
            .snapshot(passenger)
            .expect("passenger read")
            .expect("passenger snapshot");

        assert!(
            coordinator
                .apply_kinematics_if_current([
                    (
                        expected_passenger,
                        movement(passenger, Vec3::new(130.0, 64.0, 0.5)),
                    ),
                    (expected_boat, movement(boat, Vec3::new(128.5, 64.0, 0.5))),
                ])
                .expect("vehicle migration")
        );
        let moved_boat = coordinator
            .snapshot(boat)
            .expect("moved boat read")
            .expect("moved boat");
        let moved_passenger = coordinator
            .snapshot(passenger)
            .expect("moved passenger read")
            .expect("moved passenger");
        assert_eq!(moved_boat.position, Vec3::new(128.5, 64.0, 0.5));
        assert_eq!(moved_passenger.position, Vec3::new(128.0, 64.0, 0.5));
        assert_eq!(
            moved_boat.vehicle.and_then(|vehicle| vehicle.passenger),
            Some(passenger)
        );
    }

    #[test]
    fn owner_runtime_routes_exact_commands_and_returns_state_on_shutdown() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let west = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("west cow");
        let east = handle
            .spawn_authoritative(cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");
        handle
            .set_velocities([
                (west, Vec3::new(0.1, 0.0, 0.0)),
                (east, Vec3::new(0.2, 0.0, 0.0)),
            ])
            .expect("owner velocities");

        let saved = handle.save_barrier().expect("runtime save barrier");
        assert_eq!(saved.snapshots().len(), 2);
        assert_eq!(
            handle
                .snapshot(east)
                .expect("east read")
                .expect("east snapshot")
                .velocity,
            Vec3::new(0.2, 0.0, 0.0)
        );
        drop(handle);

        let recovered = runtime.shutdown().expect("runtime shutdown");
        assert_eq!(recovered.len(), 2);
        assert_eq!(
            recovered.snapshot(west).expect("recovered west").velocity,
            Vec3::new(0.1, 0.0, 0.0)
        );
    }

    #[test]
    fn journal_checkpoint_does_not_block_checkpoint_only_goal_updates() {
        let state = Arc::new(Mutex::new(TestDecisionJournalState::default()));
        let (clear_started, clear_started_rx) = mpsc::sync_channel(0);
        let (clear_release, clear_release_rx) = mpsc::sync_channel(0);
        let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
            RegionalEntityStore::new(),
            1,
            Box::new(BlockingClearDecisionJournal {
                state,
                clear_started,
                clear_release: clear_release_rx,
            }),
        )
        .expect("owner runtime");
        let handle = runtime.handle();
        let entity = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("durable cow spawn");
        let phases = handle
            .save_barrier()
            .expect("save barrier")
            .journal_phases()
            .to_vec();
        assert!(!phases.is_empty());

        let clear_handle = handle.clone();
        let clear = std::thread::spawn(move || clear_handle.clear_recovered_commits(phases));
        clear_started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("journal clear starts");

        let goal_handle = handle.clone();
        let (goal_done, goal_done_rx) = mpsc::channel();
        let goal = std::thread::spawn(move || {
            let result = goal_handle.set_goals_deferred_journal([(
                entity,
                GoalState::FollowPosition {
                    target: Vec3::new(4.5, 64.0, 0.5),
                    speed: 0.25,
                },
            )]);
            goal_done.send(result).expect("publish goal update");
        });

        let goal_before_clear_finished = goal_done_rx.recv_timeout(Duration::from_secs(1));
        clear_release.send(()).expect("release journal clear");
        assert_eq!(clear.join().expect("join journal clear"), Ok(()));
        goal.join().expect("join goal update");
        assert_eq!(
            goal_before_clear_finished
                .expect("checkpoint-only goal update must not wait for journal checkpoint I/O"),
            Ok(1)
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_selected_reads_bypass_the_coordinator_actor() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let west = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("west cow");
        let east = handle
            .spawn_authoritative(cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");
        let selected = HashSet::from([west, east]);
        assert_eq!(
            handle
                .snapshots_for_ids(&selected)
                .expect("warm direct routes")
                .len(),
            2
        );

        let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
        let (coordinator_release, coordinator_release_rx) = mpsc::channel();
        handle
            .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
            .expect("queue coordinator hold");
        coordinator_entered_rx
            .recv()
            .expect("coordinator entered hold");

        let (read_complete, read_complete_rx) = mpsc::channel();
        let read_handle = handle.clone();
        let reader = std::thread::spawn(move || {
            let result = read_handle.snapshots_for_ids(&selected);
            read_complete.send(result).expect("publish direct read");
        });
        let snapshots = read_complete_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("direct owner-lane read must not wait for coordinator")
            .expect("direct owner-lane read");
        assert_eq!(
            snapshots
                .iter()
                .map(|snapshot| snapshot.id)
                .collect::<HashSet<_>>(),
            HashSet::from([west, east])
        );

        coordinator_release.send(()).expect("release coordinator");
        reader.join().expect("join direct reader");
        handle.status().expect("coordinator resumed");
        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_point_read_bypasses_the_coordinator_actor() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let entity = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("cow");
        assert_eq!(
            handle
                .snapshot(entity)
                .expect("warm point route")
                .expect("warm cow snapshot")
                .id,
            entity
        );

        let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
        let (coordinator_release, coordinator_release_rx) = mpsc::channel();
        handle
            .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
            .expect("queue coordinator hold");
        coordinator_entered_rx
            .recv()
            .expect("coordinator entered hold");

        let (read_complete, read_complete_rx) = mpsc::channel();
        let read_handle = handle.clone();
        let reader = std::thread::spawn(move || {
            read_complete
                .send(read_handle.snapshot(entity))
                .expect("publish point read");
        });
        let direct = read_complete_rx.recv_timeout(Duration::from_secs(1));
        coordinator_release.send(()).expect("release coordinator");
        reader.join().expect("join point reader");
        let snapshot = direct
            .expect("cached point read must not wait for coordinator")
            .expect("direct point read")
            .expect("cow snapshot");
        assert_eq!(snapshot.id, entity);

        handle.status().expect("coordinator resumed");
        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_item_stack_cas_bypasses_the_coordinator_actor() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let mut item = SpawnEntity::new(1, "minecraft:item", Vec3::new(0.5, 64.0, 0.5));
        item.item_stack = Some(crate::EntityItemStack::new(7, 3));
        let entity = handle.spawn_authoritative(item).expect("item");
        let expected = handle
            .snapshot(entity)
            .expect("warm point route")
            .expect("item snapshot");

        let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
        let (coordinator_release, coordinator_release_rx) = mpsc::channel();
        handle
            .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
            .expect("queue coordinator hold");
        coordinator_entered_rx
            .recv()
            .expect("coordinator entered hold");

        let (mutation_complete, mutation_complete_rx) = mpsc::channel();
        let mutation_handle = handle.clone();
        let mutation =
            std::thread::spawn(move || {
                mutation_complete
                    .send(mutation_handle.set_item_stack_if_current(
                        expected,
                        Some(crate::EntityItemStack::new(7, 2)),
                    ))
                    .expect("publish direct item CAS");
            });
        let direct = mutation_complete_rx.recv_timeout(Duration::from_secs(1));
        coordinator_release.send(()).expect("release coordinator");
        mutation.join().expect("join item mutation");
        assert!(
            direct
                .expect("cached item CAS must not wait for coordinator")
                .expect("direct item CAS")
        );
        assert_eq!(
            handle
                .snapshot(entity)
                .expect("updated item read")
                .expect("updated item snapshot")
                .item_stack,
            Some(crate::EntityItemStack::new(7, 2))
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_single_animal_cas_bypasses_the_coordinator_actor() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let mut cow = cow(Vec3::new(0.5, 64.0, 0.5));
        cow.animal = Some(AnimalBreedingState::adult());
        let entity = handle.spawn_authoritative(cow).expect("cow");
        let expected = handle
            .snapshot(entity)
            .expect("warm point route")
            .expect("cow snapshot");
        let mut animal = expected.animal.expect("animal state");
        animal.love_ticks = 600;

        let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
        let (coordinator_release, coordinator_release_rx) = mpsc::channel();
        handle
            .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
            .expect("queue coordinator hold");
        coordinator_entered_rx
            .recv()
            .expect("coordinator entered hold");

        let (mutation_complete, mutation_complete_rx) = mpsc::channel();
        let mutation_handle = handle.clone();
        let mutation = std::thread::spawn(move || {
            mutation_complete
                .send(mutation_handle.set_animal_states_if_current([(expected, animal)]))
                .expect("publish direct animal CAS");
        });
        let direct = mutation_complete_rx.recv_timeout(Duration::from_secs(1));
        coordinator_release.send(()).expect("release coordinator");
        mutation.join().expect("join animal mutation");
        assert!(
            direct
                .expect("cached animal CAS must not wait for coordinator")
                .expect("direct animal CAS")
        );
        assert_eq!(
            handle
                .snapshot(entity)
                .expect("updated animal read")
                .expect("updated animal snapshot")
                .animal
                .expect("updated animal state")
                .love_ticks,
            600
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_same_lane_animal_batch_bypasses_the_coordinator_actor() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let mut first = cow(Vec3::new(0.5, 64.0, 0.5));
        first.animal = Some(AnimalBreedingState::adult());
        let mut second = cow(Vec3::new(1.5, 64.0, 0.5));
        second.animal = Some(AnimalBreedingState::adult());
        let first = handle.spawn_authoritative(first).expect("first cow");
        let second = handle.spawn_authoritative(second).expect("second cow");
        let selected = HashSet::from([first, second]);
        let mut expected = handle
            .snapshots_for_ids(&selected)
            .expect("warm same-lane routes");
        expected.sort_unstable_by_key(|snapshot| snapshot.id);
        let states = expected
            .into_iter()
            .map(|snapshot| {
                let mut animal = snapshot.animal.expect("animal state");
                animal.age_ticks = -20;
                (snapshot, animal)
            })
            .collect::<Vec<_>>();

        let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
        let (coordinator_release, coordinator_release_rx) = mpsc::channel();
        handle
            .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
            .expect("queue coordinator hold");
        coordinator_entered_rx
            .recv()
            .expect("coordinator entered hold");

        let (mutation_complete, mutation_complete_rx) = mpsc::channel();
        let mutation_handle = handle.clone();
        let mutation = std::thread::spawn(move || {
            mutation_complete
                .send(mutation_handle.set_animal_states_if_current(states))
                .expect("publish direct animal batch");
        });
        let direct = mutation_complete_rx.recv_timeout(Duration::from_secs(1));
        coordinator_release.send(()).expect("release coordinator");
        mutation.join().expect("join animal batch");
        assert!(
            direct
                .expect("cached same-lane animal batch must not wait for coordinator")
                .expect("direct animal batch")
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_standalone_kinematics_bypass_the_coordinator_actor() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let entity = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("cow");
        let expected = handle
            .snapshot(entity)
            .expect("warm point route")
            .expect("cow snapshot");
        let next = movement(entity, Vec3::new(1.0, 64.0, 0.5));

        let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
        let (coordinator_release, coordinator_release_rx) = mpsc::channel();
        handle
            .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
            .expect("queue coordinator hold");
        coordinator_entered_rx
            .recv()
            .expect("coordinator entered hold");

        let (mutation_complete, mutation_complete_rx) = mpsc::channel();
        let mutation_handle = handle.clone();
        let mutation = std::thread::spawn(move || {
            mutation_complete
                .send(mutation_handle.apply_kinematics_if_current([(expected, next)]))
                .expect("publish direct kinematics CAS");
        });
        let direct = mutation_complete_rx.recv_timeout(Duration::from_secs(1));
        coordinator_release.send(()).expect("release coordinator");
        mutation.join().expect("join kinematics mutation");
        assert!(
            direct
                .expect("cached standalone kinematics must not wait for coordinator")
                .expect("direct kinematics CAS")
        );
        assert_eq!(
            handle
                .snapshot(entity)
                .expect("updated kinematics read")
                .expect("updated cow snapshot")
                .position,
            next.position
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_standalone_kinematics_roll_back_safe_journal_failure() {
        let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
        let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
            RegionalEntityStore::new(),
            1,
            Box::new(TestDecisionJournal(Arc::clone(&journal))),
        )
        .expect("owner runtime");
        let handle = runtime.handle();
        let entity = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("cow");
        let expected = handle
            .snapshot(entity)
            .expect("warm point route")
            .expect("cow snapshot");
        journal.lock().expect("journal state").fail_record = true;

        assert_eq!(
            handle.apply_kinematics_if_current([(
                expected.clone(),
                movement(entity, Vec3::new(1.0, 64.0, 0.5)),
            )]),
            Err(super::RegionOwnerLaneError::Journal)
        );
        assert_eq!(
            handle
                .snapshot(entity)
                .expect("rolled back read")
                .expect("rolled back cow"),
            expected
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_standalone_kinematics_can_defer_journal() {
        let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
        let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
            RegionalEntityStore::new(),
            1,
            Box::new(TestDecisionJournal(Arc::clone(&journal))),
        )
        .expect("owner runtime");
        let handle = runtime.handle();
        let entity = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("cow");
        let expected = handle
            .snapshot(entity)
            .expect("warm point route")
            .expect("cow snapshot");
        let commit_count = journal.lock().expect("journal state").commits.len();
        let next = movement(entity, Vec3::new(1.0, 64.0, 0.5));

        assert!(
            handle
                .apply_kinematics_if_current_deferred_journal([(expected, next)])
                .expect("checkpoint-only kinematics CAS")
        );
        assert_eq!(
            journal.lock().expect("journal state").commits.len(),
            commit_count
        );
        assert_eq!(
            handle
                .snapshot(entity)
                .expect("updated read")
                .expect("updated cow")
                .position,
            next.position
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn concurrent_cached_standalone_kinematics_apply_one_exact_snapshot() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let entity = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("cow");
        let expected = handle
            .snapshot(entity)
            .expect("warm point route")
            .expect("cow snapshot");
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let first_position = Vec3::new(1.0, 64.0, 0.5);
        let second_position = Vec3::new(1.5, 64.0, 0.5);

        let (first, second) = std::thread::scope(|scope| {
            let first_handle = handle.clone();
            let first_expected = expected.clone();
            let first_barrier = Arc::clone(&barrier);
            let first = scope.spawn(move || {
                first_barrier.wait();
                first_handle.apply_kinematics_if_current([(
                    first_expected,
                    movement(entity, first_position),
                )])
            });
            let second_handle = handle.clone();
            let second_barrier = Arc::clone(&barrier);
            let second = scope.spawn(move || {
                second_barrier.wait();
                second_handle
                    .apply_kinematics_if_current([(expected, movement(entity, second_position))])
            });
            barrier.wait();
            (
                first.join().expect("first kinematics worker"),
                second.join().expect("second kinematics worker"),
            )
        });
        let applied = [
            first.expect("first kinematics"),
            second.expect("second kinematics"),
        ]
        .into_iter()
        .filter(|applied| *applied)
        .count();
        assert_eq!(applied, 1);
        assert!(
            [first_position, second_position].contains(
                &handle
                    .snapshot(entity)
                    .expect("final read")
                    .expect("final cow")
                    .position
            )
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_same_lane_non_reference_goals_bypass_the_coordinator_actor() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let first = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("first cow");
        let second = handle
            .spawn_authoritative(cow(Vec3::new(1.5, 64.0, 0.5)))
            .expect("second cow");
        let selected = HashSet::from([first, second]);
        assert_eq!(
            handle.snapshots().expect("warm full-snapshot routes").len(),
            2
        );
        let goal = GoalState::FollowPosition {
            target: Vec3::new(8.5, 64.0, 8.5),
            speed: 0.25,
        };

        let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
        let (coordinator_release, coordinator_release_rx) = mpsc::channel();
        handle
            .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
            .expect("queue coordinator hold");
        coordinator_entered_rx
            .recv()
            .expect("coordinator entered hold");

        let (mutation_complete, mutation_complete_rx) = mpsc::channel();
        let mutation_handle = handle.clone();
        let expected_goal = goal.clone();
        let mutation = std::thread::spawn(move || {
            mutation_complete
                .send(mutation_handle.set_goals([(first, goal.clone()), (second, goal)]))
                .expect("publish direct goal batch");
        });
        let direct = mutation_complete_rx.recv_timeout(Duration::from_secs(1));
        coordinator_release.send(()).expect("release coordinator");
        mutation.join().expect("join goal batch");
        assert_eq!(
            direct.expect("cached same-lane goals must not wait for coordinator"),
            Ok(2)
        );
        assert!(
            handle
                .snapshots_for_ids(&selected)
                .expect("updated goals")
                .iter()
                .all(|snapshot| snapshot.goal == expected_goal)
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_follow_target_goal_bypasses_the_coordinator_actor() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let target = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("target cow");
        let follower = handle
            .spawn_authoritative(cow(Vec3::new(1.5, 64.0, 0.5)))
            .expect("follower cow");
        let selected = HashSet::from([target, follower]);
        assert_eq!(
            handle
                .snapshots_for_ids(&selected)
                .expect("warm follower and target routes")
                .len(),
            2
        );
        let goal = GoalState::FollowTarget {
            target,
            speed: 0.25,
        };

        let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
        let (coordinator_release, coordinator_release_rx) = mpsc::channel();
        handle
            .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
            .expect("queue coordinator hold");
        coordinator_entered_rx
            .recv()
            .expect("coordinator entered hold");

        let (mutation_complete, mutation_complete_rx) = mpsc::channel();
        let mutation_handle = handle.clone();
        let expected_goal = goal.clone();
        let mutation = std::thread::spawn(move || {
            mutation_complete
                .send(mutation_handle.set_goals([(follower, goal)]))
                .expect("publish direct follow-target goal");
        });
        let direct = mutation_complete_rx.recv_timeout(Duration::from_secs(1));
        coordinator_release.send(()).expect("release coordinator");
        mutation.join().expect("join follow-target goal mutation");
        assert_eq!(
            direct.expect("cached follow-target goal must not wait for coordinator"),
            Ok(1)
        );
        assert_eq!(
            handle
                .snapshot(follower)
                .expect("updated follower read")
                .expect("follower snapshot")
                .goal,
            expected_goal
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_follow_target_goal_falls_back_after_target_migration() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let target = handle
            .spawn_authoritative(cow(Vec3::new(127.5, 64.0, 0.5)))
            .expect("target cow");
        let follower = handle
            .spawn_authoritative(cow(Vec3::new(126.5, 64.0, 0.5)))
            .expect("follower cow");
        let selected = HashSet::from([target, follower]);
        assert_eq!(
            handle
                .snapshots_for_ids(&selected)
                .expect("warm follower and target routes")
                .len(),
            2
        );

        let (read_entered, read_entered_rx) = mpsc::channel();
        let (read_release, read_release_rx) = mpsc::channel();
        handle.pause_selected_read_after_dispatch_for_test(read_entered, read_release_rx);
        let (fallback_entered, fallback_entered_rx) = mpsc::channel();
        handle.notify_referenced_goal_fallback_for_test(fallback_entered);
        let mutation_handle = handle.clone();
        let mutation = std::thread::spawn(move || {
            mutation_handle.set_goals([(
                follower,
                GoalState::FollowTarget {
                    target,
                    speed: 0.25,
                },
            )])
        });
        if let Err(error) = read_entered_rx.recv_timeout(Duration::from_secs(1)) {
            *handle
                .selected_read_probe
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            let result = mutation.join().expect("join missing direct read");
            panic!("follow-target selected read was not dispatched: {error}; result: {result:?}");
        }

        assert!(
            handle
                .set_position(target, Vec3::new(128.5, 64.0, 0.5))
                .expect("migrate target")
        );

        let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
        let (coordinator_release, coordinator_release_rx) = mpsc::channel();
        handle
            .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
            .expect("queue coordinator hold");
        coordinator_entered_rx
            .recv()
            .expect("coordinator entered hold");
        read_release.send(()).expect("release selected read");
        fallback_entered_rx
            .recv()
            .expect("version fence rejected direct follow-target goal");
        assert_eq!(
            handle
                .snapshot(follower)
                .expect("follower read before fallback")
                .expect("follower snapshot")
                .goal,
            GoalState::Idle
        );
        coordinator_release.send(()).expect("release coordinator");
        assert_eq!(mutation.join().expect("join follow-target fallback"), Ok(1));
        assert_eq!(
            handle
                .snapshot(follower)
                .expect("follower read after fallback")
                .expect("follower snapshot")
                .goal,
            GoalState::FollowTarget {
                target,
                speed: 0.25,
            }
        );
        assert_eq!(
            handle
                .snapshot(target)
                .expect("migrated target read")
                .expect("target snapshot")
                .position,
            Vec3::new(128.5, 64.0, 0.5)
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_same_lane_animal_batch_records_one_durable_decision() {
        let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
        let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
            RegionalEntityStore::new(),
            2,
            Box::new(TestDecisionJournal(Arc::clone(&journal))),
        )
        .expect("owner runtime");
        let handle = runtime.handle();
        let mut first = cow(Vec3::new(0.5, 64.0, 0.5));
        first.animal = Some(AnimalBreedingState::adult());
        let mut second = cow(Vec3::new(1.5, 64.0, 0.5));
        second.animal = Some(AnimalBreedingState::adult());
        let ids = [
            handle.spawn_authoritative(first).expect("first cow"),
            handle.spawn_authoritative(second).expect("second cow"),
        ];
        let mut expected = handle
            .snapshots_for_ids(&HashSet::from(ids))
            .expect("warm same-lane routes");
        expected.sort_unstable_by_key(|snapshot| snapshot.id);
        let states = expected
            .into_iter()
            .map(|snapshot| {
                let mut animal = snapshot.animal.expect("animal state");
                animal.age_ticks = -20;
                (snapshot, animal)
            })
            .collect::<Vec<_>>();

        assert!(
            handle
                .set_animal_states_if_current(states)
                .expect("direct animal batch")
        );

        let state = journal.lock().expect("journal state");
        let decision = state.commits.last().expect("batch durable decision");
        assert_eq!(decision.upserts().len(), 2);
        assert!(decision.upserts().iter().all(|snapshot| {
            snapshot
                .animal
                .is_some_and(|animal| animal.age_ticks == -20)
        }));
        drop(state);

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_same_lane_animal_batch_can_defer_journal() {
        let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
        let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
            RegionalEntityStore::new(),
            2,
            Box::new(TestDecisionJournal(Arc::clone(&journal))),
        )
        .expect("owner runtime");
        let handle = runtime.handle();
        let mut animal = cow(Vec3::new(0.5, 64.0, 0.5));
        animal.animal = Some(AnimalBreedingState::baby());
        let id = handle.spawn_authoritative(animal).expect("baby cow");
        let expected = handle
            .snapshot(id)
            .expect("warm point route")
            .expect("baby cow snapshot");
        let commit_count = journal.lock().expect("journal state").commits.len();
        let mut next = expected.animal.expect("animal state");
        next.age_ticks += 1;

        assert!(
            handle
                .set_animal_states_if_current_deferred_journal([(expected, next)])
                .expect("checkpoint-only animal CAS")
        );

        assert_eq!(
            journal.lock().expect("journal state").commits.len(),
            commit_count
        );
        assert_eq!(
            handle
                .snapshot(id)
                .expect("updated point read")
                .expect("baby cow remains")
                .animal,
            Some(next)
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_same_lane_animal_batch_rolls_back_safe_journal_failure() {
        let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
        let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
            RegionalEntityStore::new(),
            1,
            Box::new(TestDecisionJournal(Arc::clone(&journal))),
        )
        .expect("owner runtime");
        let handle = runtime.handle();
        let mut first = cow(Vec3::new(0.5, 64.0, 0.5));
        first.animal = Some(AnimalBreedingState::adult());
        let mut second = cow(Vec3::new(1.5, 64.0, 0.5));
        second.animal = Some(AnimalBreedingState::adult());
        let ids = [
            handle.spawn_authoritative(first).expect("first cow"),
            handle.spawn_authoritative(second).expect("second cow"),
        ];
        let selected = HashSet::from(ids);
        let states = handle
            .snapshots_for_ids(&selected)
            .expect("warm same-lane routes")
            .into_iter()
            .map(|snapshot| {
                let mut animal = snapshot.animal.expect("animal state");
                animal.age_ticks = -20;
                (snapshot, animal)
            })
            .collect::<Vec<_>>();
        journal.lock().expect("journal state").fail_record = true;

        assert_eq!(
            handle.set_animal_states_if_current(states),
            Err(super::RegionOwnerLaneError::Journal)
        );
        assert!(
            handle
                .snapshots_for_ids(&selected)
                .expect("rolled back snapshots")
                .iter()
                .all(|snapshot| snapshot.animal.is_some_and(|animal| animal.age_ticks == 0))
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_damage_cas_bypasses_the_coordinator_actor() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let entity = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("cow");
        let expected = handle
            .snapshot(entity)
            .expect("warm point route")
            .expect("cow snapshot");

        let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
        let (coordinator_release, coordinator_release_rx) = mpsc::channel();
        handle
            .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
            .expect("queue coordinator hold");
        coordinator_entered_rx
            .recv()
            .expect("coordinator entered hold");

        let (mutation_complete, mutation_complete_rx) = mpsc::channel();
        let mutation_handle = handle.clone();
        let mutation = std::thread::spawn(move || {
            mutation_complete
                .send(mutation_handle.damage_if_current(expected, 1.0))
                .expect("publish direct damage CAS");
        });
        let direct = mutation_complete_rx.recv_timeout(Duration::from_secs(1));
        coordinator_release.send(()).expect("release coordinator");
        mutation.join().expect("join damage mutation");
        let damage = direct
            .expect("cached damage CAS must not wait for coordinator")
            .expect("direct damage CAS")
            .expect("damage applied");
        assert_eq!(damage.snapshot.id, entity);
        assert!(damage.snapshot.health < 20.0);
        assert!(!damage.killed);

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_damage_cas_rolls_back_safe_journal_failure() {
        let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
        let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
            RegionalEntityStore::new(),
            1,
            Box::new(TestDecisionJournal(Arc::clone(&journal))),
        )
        .expect("owner runtime");
        let handle = runtime.handle();
        let entity = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("cow");
        let expected = handle
            .snapshot(entity)
            .expect("warm point route")
            .expect("cow snapshot");
        let health = expected.health;
        journal.lock().expect("journal state").fail_record = true;

        assert_eq!(
            handle.damage_if_current(expected, 1.0),
            Err(super::RegionOwnerLaneError::Journal)
        );
        assert_eq!(
            handle
                .snapshot(entity)
                .expect("rolled back read")
                .expect("rolled back cow")
                .health,
            health
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_damage_cas_reports_lethal_post_state() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
            .expect("owner runtime");
        let handle = runtime.handle();
        let entity = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("cow");
        let expected = handle
            .snapshot(entity)
            .expect("warm point route")
            .expect("cow snapshot");

        let damage = handle
            .damage_if_current(expected, 100.0)
            .expect("direct lethal damage")
            .expect("damage applied");
        assert!(damage.killed);
        assert_eq!(
            damage.snapshot.lifecycle,
            crate::EntityLifecycle::Despawning
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn concurrent_cached_damage_cas_applies_one_exact_snapshot() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let entity = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("cow");
        let expected = handle
            .snapshot(entity)
            .expect("warm point route")
            .expect("cow snapshot");
        let initial_health = expected.health;
        let barrier = Arc::new(std::sync::Barrier::new(3));

        let (first, second) = std::thread::scope(|scope| {
            let first_handle = handle.clone();
            let first_expected = expected.clone();
            let first_barrier = Arc::clone(&barrier);
            let first = scope.spawn(move || {
                first_barrier.wait();
                first_handle.damage_if_current(first_expected, 1.0)
            });
            let second_handle = handle.clone();
            let second_barrier = Arc::clone(&barrier);
            let second = scope.spawn(move || {
                second_barrier.wait();
                second_handle.damage_if_current(expected, 1.0)
            });
            barrier.wait();
            (
                first.join().expect("first damage worker"),
                second.join().expect("second damage worker"),
            )
        });
        let applied = [first.expect("first damage"), second.expect("second damage")]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        assert_eq!(applied.len(), 1);
        assert!(applied[0].snapshot.health < initial_health);
        assert_eq!(
            handle
                .snapshot(entity)
                .expect("final read")
                .expect("final cow")
                .health,
            applied[0].snapshot.health
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_item_stack_cas_records_post_state_and_save_phase() {
        let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
        let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
            RegionalEntityStore::new(),
            2,
            Box::new(TestDecisionJournal(Arc::clone(&journal))),
        )
        .expect("owner runtime");
        let handle = runtime.handle();
        let mut item = SpawnEntity::new(1, "minecraft:item", Vec3::new(0.5, 64.0, 0.5));
        item.item_stack = Some(crate::EntityItemStack::new(7, 3));
        let entity = handle.spawn_authoritative(item).expect("item");
        let expected = handle
            .snapshot(entity)
            .expect("warm point route")
            .expect("item snapshot");

        assert!(
            handle
                .set_item_stack_if_current(expected, Some(crate::EntityItemStack::new(7, 2)),)
                .expect("direct item CAS")
        );

        let state = journal.lock().expect("journal state");
        let decision = state.commits.last().expect("direct durable decision");
        assert_eq!(decision.upserts().len(), 1);
        assert_eq!(decision.upserts()[0].id, entity);
        assert_eq!(
            decision.upserts()[0].item_stack,
            Some(crate::EntityItemStack::new(7, 2))
        );
        let phase = decision.phase();
        drop(state);
        assert!(
            handle
                .save_barrier()
                .expect("save barrier")
                .journal_phases()
                .contains(&phase)
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn cached_item_stack_cas_rolls_back_safe_journal_failure() {
        let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
        let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
            RegionalEntityStore::new(),
            1,
            Box::new(TestDecisionJournal(Arc::clone(&journal))),
        )
        .expect("owner runtime");
        let handle = runtime.handle();
        let mut item = SpawnEntity::new(1, "minecraft:item", Vec3::new(0.5, 64.0, 0.5));
        item.item_stack = Some(crate::EntityItemStack::new(7, 3));
        let entity = handle.spawn_authoritative(item).expect("item");
        let expected = handle
            .snapshot(entity)
            .expect("warm point route")
            .expect("item snapshot");
        journal.lock().expect("journal state").fail_record = true;

        assert_eq!(
            handle.set_item_stack_if_current(expected, Some(crate::EntityItemStack::new(7, 2)),),
            Err(super::RegionOwnerLaneError::Journal)
        );
        assert_eq!(
            handle
                .snapshot(entity)
                .expect("item read")
                .expect("item snapshot")
                .item_stack,
            Some(crate::EntityItemStack::new(7, 3))
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn direct_selected_read_admission_reserves_lane_queue_capacity() {
        let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut permits = (0..super::DIRECT_SELECTED_READ_LIMIT)
            .map(|_| {
                super::DirectSelectedReadPermit::try_acquire(Arc::clone(&active))
                    .expect("admitted direct read")
            })
            .collect::<Vec<_>>();
        assert!(super::DirectSelectedReadPermit::try_acquire(Arc::clone(&active)).is_none());
        permits.pop();
        assert!(super::DirectSelectedReadPermit::try_acquire(active).is_some());
    }

    #[test]
    fn cached_multi_lane_read_retries_after_overlapping_mutation() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let west = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("west cow");
        let east = handle
            .spawn_authoritative(cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");
        let selected = HashSet::from([west, east]);
        assert_eq!(
            handle
                .snapshots_for_ids(&selected)
                .expect("warm direct routes")
                .len(),
            2
        );

        let (read_entered, read_entered_rx) = mpsc::channel();
        let (read_release, read_release_rx) = mpsc::channel();
        handle.pause_selected_read_after_dispatch_for_test(read_entered, read_release_rx);
        let read_handle = handle.clone();
        let reader = std::thread::spawn(move || read_handle.snapshots_for_ids(&selected));
        read_entered_rx
            .recv()
            .expect("selected read dispatched to both lanes");

        handle
            .set_velocities([
                (west, Vec3::new(0.1, 0.0, 0.0)),
                (east, Vec3::new(0.2, 0.0, 0.0)),
            ])
            .expect("overlapping regional mutation");
        read_release.send(()).expect("release selected read");
        let snapshots = reader
            .join()
            .expect("join selected reader")
            .expect("selected read fallback");
        assert_eq!(snapshots.len(), 2);
        assert_eq!(snapshots[0].id, west);
        assert_eq!(snapshots[0].velocity, Vec3::new(0.1, 0.0, 0.0));
        assert_eq!(snapshots[1].id, east);
        assert_eq!(snapshots[1].velocity, Vec3::new(0.2, 0.0, 0.0));

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn selected_read_route_refreshes_after_region_migration() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let entity = handle
            .spawn_authoritative(cow(Vec3::new(127.5, 64.0, 0.5)))
            .expect("boundary cow");
        let selected = HashSet::from([entity]);
        let expected = handle
            .snapshots_for_ids(&selected)
            .expect("warm source route")
            .pop()
            .expect("source snapshot");
        assert!(
            handle
                .apply_kinematics_if_current([(
                    expected,
                    movement(entity, Vec3::new(128.5, 64.0, 0.5)),
                )])
                .expect("cross-region movement")
        );
        assert_eq!(
            handle
                .snapshots_for_ids(&selected)
                .expect("fallback and refresh target route")[0]
                .position,
            Vec3::new(128.5, 64.0, 0.5)
        );

        let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
        let (coordinator_release, coordinator_release_rx) = mpsc::channel();
        handle
            .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
            .expect("queue coordinator hold");
        coordinator_entered_rx
            .recv()
            .expect("coordinator entered hold");
        let snapshot = handle
            .read_cached_selected_entities(&selected)
            .expect("refreshed target route remains direct")
            .pop()
            .expect("target snapshot");
        assert_eq!(snapshot.position, Vec3::new(128.5, 64.0, 0.5));

        coordinator_release.send(()).expect("release coordinator");
        handle.status().expect("coordinator resumed");
        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn owner_runtime_routes_goal_and_conditional_physics_commands() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let mut west = cow(Vec3::new(0.5, 64.0, 0.5));
        west.goal = GoalState::Wander {
            speed: 0.2,
            period_ticks: 20,
        };
        let west = handle.spawn_authoritative(west).expect("west cow");
        let east = handle
            .spawn_authoritative(cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");

        let prepared = handle
            .prepare_goal_tick_with_pathing_for_ids(17, &HashSet::from([west]))
            .expect("prepare runtime goal");
        let stats = handle
            .apply_prepared_goal_tick(prepared.resolve(&WalkablePathing, PathingBudget::DEFAULT))
            .expect("apply runtime goal");
        assert_eq!(stats.decisions_applied, 1);

        let expected = handle
            .snapshot(east)
            .expect("east read")
            .expect("east snapshot");
        assert!(
            handle
                .apply_kinematics_if_current([(
                    expected,
                    movement(east, Vec3::new(129.0, 64.0, 0.5)),
                )])
                .expect("runtime physics")
        );
        assert_eq!(
            handle
                .snapshot(east)
                .expect("moved read")
                .expect("moved snapshot")
                .position,
            Vec3::new(129.0, 64.0, 0.5)
        );
        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn owner_runtime_reloads_a_stale_versioned_goal_input() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
            .expect("owner runtime");
        let handle = runtime.handle();
        let follower = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("follower");
        assert!(
            handle
                .set_goal(
                    follower,
                    GoalState::FollowPosition {
                        target: Vec3::new(4.5, 64.0, 0.5),
                        speed: 0.2,
                    },
                )
                .expect("set follower goal")
        );
        let active = HashSet::from([follower]);
        let selected = handle
            .snapshots_for_ids_versioned(&active)
            .expect("versioned follower input");
        assert!(
            handle
                .set_position(follower, Vec3::new(1.5, 64.0, 0.5))
                .expect("move follower")
        );

        let prepared = handle
            .prepare_goal_tick_with_pathing_for_versioned_snapshots(17, &active, selected)
            .expect("prepare from stale input");
        let stats = handle
            .apply_prepared_goal_tick(prepared.resolve(&WalkablePathing, PathingBudget::DEFAULT))
            .expect("apply fresh goal input");

        assert_eq!(stats.decisions_applied, 1);
        let snapshot = handle
            .snapshot(follower)
            .expect("follower read")
            .expect("follower snapshot");
        assert_eq!(snapshot.position, Vec3::new(1.5, 64.0, 0.5));
        assert_ne!(snapshot.velocity, Vec3::ZERO);
        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn owner_runtime_routes_common_gameplay_mutations() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let cow_id = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("cow");
        assert!(
            handle
                .set_goal(
                    cow_id,
                    GoalState::FollowPosition {
                        target: Vec3::new(3.5, 64.0, 0.5),
                        speed: 0.25,
                    },
                )
                .expect("set goal")
        );
        assert!(
            handle
                .set_position(cow_id, Vec3::new(1.5, 64.0, 0.5))
                .expect("set position")
        );
        assert_eq!(
            handle
                .damage(cow_id, 5.0)
                .expect("damage")
                .expect("damage result")
                .snapshot
                .health,
            15.0
        );

        let mut item = SpawnEntity::new(41, "minecraft:item", Vec3::new(2.5, 64.0, 0.5));
        item.item_stack = Some(crate::EntityItemStack::new(7, 1));
        let item = handle.spawn_authoritative(item).expect("item");
        assert!(
            handle
                .set_item_stack(item, Some(crate::EntityItemStack::new(7, 2)))
                .expect("set item stack")
        );
        assert_eq!(
            handle
                .snapshot(item)
                .expect("item read")
                .expect("item snapshot")
                .item_stack,
            Some(crate::EntityItemStack::new(7, 2))
        );
        let herd = handle
            .spawn_authoritative_batch([
                cow(Vec3::new(3.5, 64.0, 0.5)),
                cow(Vec3::new(128.5, 64.0, 0.5)),
            ])
            .expect("spawn herd");
        assert_eq!(herd.len(), 2);
        assert_eq!(
            handle
                .set_goals(herd.iter().copied().map(|entity| {
                    (
                        entity,
                        GoalState::Wander {
                            speed: 0.15,
                            period_ticks: 40,
                        },
                    )
                }))
                .expect("set herd goals"),
            2
        );
        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn owner_unique_batch_skips_duplicate_input_and_restored_uuids() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let restored_uuid = Uuid::from_u128(0x11);
        let new_uuid = Uuid::from_u128(0x22);
        let mut restored_store = EntityStore::new();
        let mut restored = cow(Vec3::new(0.5, 64.0, 0.5));
        restored.uuid = Some(restored_uuid);
        let restored_id = restored_store.spawn_authoritative(restored);
        let restored = restored_store
            .snapshot(restored_id)
            .expect("restored snapshot");
        assert_eq!(
            handle.insert_authoritative_snapshots_batch([restored.clone()]),
            Ok(1)
        );

        let mut existing_duplicate = cow(Vec3::new(1.5, 64.0, 0.5));
        existing_duplicate.uuid = Some(restored_uuid);
        let mut new = cow(Vec3::new(2.5, 64.0, 0.5));
        new.uuid = Some(new_uuid);
        let mut input_duplicate = cow(Vec3::new(3.5, 64.0, 0.5));
        input_duplicate.uuid = Some(new_uuid);

        let committed = handle
            .spawn_unique_authoritative_batch([existing_duplicate, new, input_duplicate])
            .expect("unique authoritative batch");

        assert_eq!(committed.len(), 1);
        assert_eq!(committed[0].uuid, new_uuid);
        assert_eq!(committed[0].position, Vec3::new(2.5, 64.0, 0.5));
        assert_eq!(handle.status().expect("owner status").entity_count, 2);
        assert_eq!(
            handle
                .snapshot(restored_id)
                .expect("restored read")
                .expect("restored entity"),
            restored
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn owner_unique_batch_journals_once_and_returns_exact_durable_snapshots() {
        let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
        let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
            RegionalEntityStore::new(),
            2,
            Box::new(TestDecisionJournal(Arc::clone(&journal))),
        )
        .expect("owner runtime");
        let handle = runtime.handle();
        let mut west = cow(Vec3::new(0.5, 64.0, 0.5));
        west.uuid = Some(Uuid::from_u128(0x33));
        let mut east = cow(Vec3::new(128.5, 64.0, 0.5));
        east.uuid = Some(Uuid::from_u128(0x44));

        let committed = handle
            .spawn_unique_authoritative_batch([west, east])
            .expect("unique authoritative batch");

        let state = journal.lock().expect("journal state");
        assert_eq!(state.commits.len(), 1);
        let decision = state.commits[0].clone();
        assert_eq!(decision.upserts(), committed.as_slice());
        assert!(decision.removed().is_empty());
        drop(state);
        let saved = handle.save_barrier().expect("owner save barrier");
        assert_eq!(saved.snapshots(), committed.as_slice());
        assert!(saved.journal_phases().contains(&decision.phase()));

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn owner_unique_batch_outcome_unknown_enters_fail_stop_without_global_publication() {
        let journal = Arc::new(Mutex::new(TestDecisionJournalState {
            fail_record_unknown: true,
            ..TestDecisionJournalState::default()
        }));
        let mut coordinator = super::RegionalOwnerCoordinator::from_store_with_journal(
            RegionalEntityStore::new(),
            1,
            Box::new(TestDecisionJournal(Arc::clone(&journal))),
        )
        .expect("owner coordinator");
        let mut uncertain = cow(Vec3::new(0.5, 64.0, 0.5));
        uncertain.uuid = Some(Uuid::from_u128(0x55));

        assert_eq!(
            coordinator.spawn_unique_authoritative_batch([uncertain]),
            Err(super::RegionOwnerLaneError::Journal)
        );
        assert_eq!(coordinator.status().entity_count, 0);
        let mut rejected = cow(Vec3::new(1.5, 64.0, 0.5));
        rejected.uuid = Some(Uuid::from_u128(0x66));
        assert_eq!(
            coordinator.spawn_unique_authoritative_batch([rejected]),
            Err(super::RegionOwnerLaneError::Busy)
        );
        let state = journal.lock().expect("journal state");
        assert_eq!(state.commits.len(), 1);
        assert_eq!(state.commits[0].upserts()[0].uuid, Uuid::from_u128(0x55));
    }

    #[test]
    fn owner_runtime_exposes_cutover_reads_and_conditional_remove() {
        let runtime =
            super::RegionalOwnerRuntime::from_store(RegionalEntityStore::with_next_id(6_000), 2)
                .expect("owner runtime");
        let handle = runtime.handle();
        let west = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("west cow");
        let east = handle
            .spawn_authoritative(cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");
        assert_eq!(west, EntityId(6_001));
        assert_eq!(east, EntityId(6_002));

        let selected = handle
            .snapshots_for_ids(&HashSet::from([east]))
            .expect("selected snapshots");
        assert_eq!(
            selected
                .iter()
                .map(|snapshot| snapshot.id)
                .collect::<Vec<_>>(),
            vec![east]
        );
        let status = handle.status().expect("owner status");
        assert_eq!(status.entity_count, 2);
        assert_eq!(status.lane_count, 2);
        assert_eq!(status.shadow.comparisons, 0);
        assert!(
            handle
                .contains_uuid(
                    handle
                        .snapshot(east)
                        .expect("east UUID read")
                        .expect("east UUID snapshot")
                        .uuid,
                )
                .expect("contains UUID")
        );

        let mut breeding_cow = cow(Vec3::new(4.5, 64.0, 0.5));
        breeding_cow.animal = Some(AnimalBreedingState {
            age_ticks: -20,
            love_ticks: 0,
            sheep_wool: None,
        });
        let breeding_cow = handle
            .spawn_authoritative(breeding_cow)
            .expect("breeding cow");
        assert_eq!(
            handle
                .breeding_tick_snapshots()
                .expect("breeding owner query")
                .into_iter()
                .map(|snapshot| snapshot.id)
                .collect::<Vec<_>>(),
            vec![breeding_cow]
        );

        let stale = handle
            .snapshot(west)
            .expect("west read")
            .expect("west snapshot");
        assert!(
            handle
                .set_position(west, Vec3::new(1.5, 64.0, 0.5))
                .expect("move west")
        );
        assert_eq!(
            handle
                .remove_if_current(stale)
                .expect("stale conditional remove"),
            None
        );
        let current = handle
            .snapshot(west)
            .expect("current west read")
            .expect("current west snapshot");
        assert_eq!(
            handle
                .remove_if_current(current.clone())
                .expect("fresh conditional remove"),
            Some(current)
        );
        assert_eq!(handle.status().expect("final owner status").entity_count, 2);

        let item = handle
            .spawn_authoritative({
                let mut item = SpawnEntity::new(41, "minecraft:item", Vec3::new(2.5, 64.0, 0.5));
                item.item_stack = Some(crate::EntityItemStack::new(7, 4));
                item
            })
            .expect("item");
        let stale_item = handle
            .snapshot(item)
            .expect("item read")
            .expect("item snapshot");
        assert!(
            handle
                .set_item_stack(item, Some(crate::EntityItemStack::new(7, 3)))
                .expect("change item")
        );
        assert!(
            !handle
                .set_item_stack_if_current(stale_item, Some(crate::EntityItemStack::new(7, 2)),)
                .expect("stale item CAS")
        );
        let current_item = handle
            .snapshot(item)
            .expect("current item read")
            .expect("current item snapshot");
        assert!(
            handle
                .set_item_stack_if_current(current_item, Some(crate::EntityItemStack::new(7, 2)),)
                .expect("fresh item CAS")
        );

        let mut vehicle = SpawnEntity::vehicle(
            VehicleKind::Boat,
            15,
            "minecraft:oak_boat",
            Vec3::new(3.5, 64.0, 0.5),
        );
        vehicle.vehicle.as_mut().expect("vehicle state").passenger = Some(EntityId(6_006));
        let mounted = handle
            .spawn_authoritative_batch([vehicle, cow(Vec3::new(3.75, 64.0, 0.5))])
            .expect("mounted pair");
        let passenger = handle
            .snapshot(mounted[1])
            .expect("passenger read")
            .expect("passenger snapshot");
        assert_eq!(
            handle.remove_if_current(passenger),
            Err(super::RegionOwnerLaneError::InvalidMutation)
        );

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn owner_runtime_reconfigures_lanes_through_the_actor() {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
            .expect("owner runtime");
        let handle = runtime.handle();
        let west = handle
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("west cow");
        let east = handle
            .spawn_authoritative(cow(Vec3::new(128.5, 64.0, 0.5)))
            .expect("east cow");
        let selected = HashSet::from([west, east]);
        assert_eq!(
            handle
                .snapshots_for_ids(&selected)
                .expect("warm single-lane routes")
                .len(),
            2
        );
        assert_eq!(
            handle
                .selected_read_routes
                .read()
                .expect("selected route cache")
                .len(),
            2
        );

        assert_eq!(handle.reconfigure_lanes(2), Ok(2));
        assert!(
            handle
                .selected_read_routes
                .read()
                .expect("cleared scale-up routes")
                .is_empty()
        );
        assert_eq!(handle.status().expect("status").lane_count, 2);
        assert_eq!(
            handle
                .snapshots_for_ids(&selected)
                .expect("selected snapshots")
                .len(),
            2
        );
        assert_eq!(handle.reconfigure_lanes(1), Ok(1));
        assert!(
            handle
                .selected_read_routes
                .read()
                .expect("cleared scale-down routes")
                .is_empty()
        );
        assert_eq!(handle.status().expect("status").lane_count, 1);
        assert_eq!(
            handle
                .snapshots_for_ids(&selected)
                .expect("selected snapshots after scale-down")
                .len(),
            2
        );

        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    #[ignore = "explicit debug persistent regional owner scaling benchmark"]
    fn persistent_owner_lane_scaling_benchmark_report() {
        const REGIONS: usize = 8;
        const ENTITIES_PER_REGION: usize = 256;
        const ITERATIONS: usize = 80;

        fn run(lanes: usize, active_regions: usize) -> (Vec<u128>, Vec<crate::EntitySnapshot>) {
            let runtime =
                super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), lanes)
                    .expect("owner runtime");
            let handle = runtime.handle();
            let entities = (0..REGIONS)
                .flat_map(|region| {
                    (0..ENTITIES_PER_REGION).map(move |index| {
                        let x = region as f64 * 128.0 + 0.5 + (index % 16) as f64;
                        let z = 0.5 + (index / 16) as f64;
                        cow(Vec3::new(x, 64.0, z))
                    })
                })
                .collect::<Vec<_>>();
            let ids = handle
                .spawn_authoritative_batch(entities)
                .expect("benchmark entities");
            let active_ids = ids
                .into_iter()
                .take(active_regions * ENTITIES_PER_REGION)
                .collect::<HashSet<_>>();

            let mut samples = Vec::with_capacity(ITERATIONS);
            for iteration in 0..ITERATIONS {
                let expected = handle
                    .snapshots_for_ids(&active_ids)
                    .expect("benchmark active snapshots");
                let offset = if iteration % 2 == 0 { 0.01 } else { -0.01 };
                let states = expected.iter().cloned().map(|snapshot| {
                    let mut state = movement(
                        snapshot.id,
                        Vec3::new(
                            snapshot.position.x + offset,
                            snapshot.position.y,
                            snapshot.position.z,
                        ),
                    );
                    state.rotation = snapshot.rotation;
                    state.velocity = snapshot.velocity;
                    state.on_ground = snapshot.on_ground;
                    (snapshot, state)
                });
                let started = Instant::now();
                assert!(
                    handle
                        .apply_kinematics_if_current(states)
                        .expect("benchmark kinematics")
                );
                samples.push(started.elapsed().as_micros());
            }
            let snapshots = handle.snapshots().expect("final snapshots");
            drop(handle);
            runtime.shutdown().expect("benchmark shutdown");
            (samples, snapshots)
        }

        fn percentile(samples: &[u128], percentile: usize) -> u128 {
            samples[(samples.len() * percentile).div_ceil(100).saturating_sub(1)]
        }

        let parallel_lanes = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(4);
        #[cfg(target_os = "linux")]
        assert_benchmark_has_distinct_physical_cores(parallel_lanes);
        let (mut serial, serial_state) = run(1, REGIONS);
        let (mut parallel, parallel_state) = run(parallel_lanes, REGIONS);
        assert_eq!(serial_state, parallel_state);
        serial.sort_unstable();
        parallel.sort_unstable();
        println!(
            "REGIONAL_OWNER_SCALING_BENCH entities={} regions={REGIONS} iterations={ITERATIONS} parallel_lanes={parallel_lanes} serial_p50_us={} serial_p95_us={} serial_p99_us={} serial_max_us={} parallel_p50_us={} parallel_p95_us={} parallel_p99_us={} parallel_max_us={}",
            REGIONS * ENTITIES_PER_REGION,
            percentile(&serial, 50),
            percentile(&serial, 95),
            percentile(&serial, 99),
            serial.last().copied().unwrap_or_default(),
            percentile(&parallel, 50),
            percentile(&parallel, 95),
            percentile(&parallel, 99),
            parallel.last().copied().unwrap_or_default(),
        );

        let active_regions = 2;
        let (mut active_serial, active_serial_state) = run(1, active_regions);
        let (mut active_parallel, active_parallel_state) = run(parallel_lanes, active_regions);
        assert_eq!(active_serial_state, active_parallel_state);
        active_serial.sort_unstable();
        active_parallel.sort_unstable();
        println!(
            "REGIONAL_OWNER_ACTIVE_SUBSET_BENCH entities={} active_entities={} regions={REGIONS} active_regions={active_regions} iterations={ITERATIONS} parallel_lanes={parallel_lanes} serial_p50_us={} serial_p95_us={} serial_p99_us={} serial_max_us={} parallel_p50_us={} parallel_p95_us={} parallel_p99_us={} parallel_max_us={}",
            REGIONS * ENTITIES_PER_REGION,
            active_regions * ENTITIES_PER_REGION,
            percentile(&active_serial, 50),
            percentile(&active_serial, 95),
            percentile(&active_serial, 99),
            active_serial.last().copied().unwrap_or_default(),
            percentile(&active_parallel, 50),
            percentile(&active_parallel, 95),
            percentile(&active_parallel, 99),
            active_parallel.last().copied().unwrap_or_default(),
        );
    }

    #[test]
    #[ignore = "explicit debug regional coordinator actor overhead benchmark"]
    fn regional_coordinator_actor_overhead_benchmark_report() {
        const REGIONS: usize = 2;
        const ENTITIES_PER_REGION: usize = 256;
        const ITERATIONS: usize = 80;

        fn entities() -> Vec<SpawnEntity> {
            (0..REGIONS)
                .flat_map(|region| {
                    (0..ENTITIES_PER_REGION).map(move |index| {
                        let x = region as f64 * 128.0 + 0.5 + (index % 16) as f64;
                        let z = 0.5 + (index / 16) as f64;
                        cow(Vec3::new(x, 64.0, z))
                    })
                })
                .collect()
        }

        fn moved(
            expected: Vec<crate::EntitySnapshot>,
            iteration: usize,
        ) -> Vec<(crate::EntitySnapshot, EntityKinematics)> {
            let offset = if iteration.is_multiple_of(2) {
                0.01
            } else {
                -0.01
            };
            expected
                .into_iter()
                .map(|snapshot| {
                    let mut state = movement(
                        snapshot.id,
                        Vec3::new(
                            snapshot.position.x + offset,
                            snapshot.position.y,
                            snapshot.position.z,
                        ),
                    );
                    state.rotation = snapshot.rotation;
                    state.velocity = snapshot.velocity;
                    state.on_ground = snapshot.on_ground;
                    (snapshot, state)
                })
                .collect()
        }

        fn direct(lanes: usize) -> (Vec<u128>, Vec<crate::EntitySnapshot>) {
            let mut coordinator =
                super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), lanes)
                    .expect("direct coordinator");
            coordinator
                .spawn_authoritative_batch(entities())
                .expect("direct benchmark entities");
            let mut samples = Vec::with_capacity(ITERATIONS);
            for iteration in 0..ITERATIONS {
                let expected = coordinator.snapshots().expect("direct snapshots");
                let started = Instant::now();
                assert!(
                    coordinator
                        .apply_kinematics_if_current(moved(expected, iteration))
                        .expect("direct kinematics")
                );
                samples.push(started.elapsed().as_micros());
            }
            let snapshots = coordinator.snapshots().expect("direct final snapshots");
            coordinator.shutdown().expect("direct shutdown");
            (samples, snapshots)
        }

        fn raw_ecs() -> (Vec<u128>, Vec<crate::EntitySnapshot>) {
            let mut store = EntityStore::new();
            store.spawn_authoritative_batch(entities());
            let mut samples = Vec::with_capacity(ITERATIONS);
            for iteration in 0..ITERATIONS {
                let expected = store.snapshots().collect::<Vec<_>>();
                let states = moved(expected, iteration)
                    .into_iter()
                    .map(|(_, state)| state)
                    .collect::<Vec<_>>();
                let started = Instant::now();
                assert_eq!(
                    store.apply_kinematics(states),
                    REGIONS * ENTITIES_PER_REGION
                );
                samples.push(started.elapsed().as_micros());
            }
            (samples, store.snapshots().collect())
        }

        fn actor(lanes: usize) -> (Vec<u128>, Vec<crate::EntitySnapshot>) {
            let runtime =
                super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), lanes)
                    .expect("actor runtime");
            let handle = runtime.handle();
            handle
                .spawn_authoritative_batch(entities())
                .expect("actor benchmark entities");
            let mut samples = Vec::with_capacity(ITERATIONS);
            for iteration in 0..ITERATIONS {
                let expected = handle.snapshots().expect("actor snapshots");
                let started = Instant::now();
                assert!(
                    handle
                        .apply_kinematics_if_current(moved(expected, iteration))
                        .expect("actor kinematics")
                );
                samples.push(started.elapsed().as_micros());
            }
            let snapshots = handle.snapshots().expect("actor final snapshots");
            drop(handle);
            runtime.shutdown().expect("actor shutdown");
            (samples, snapshots)
        }

        fn percentile(samples: &[u128], percentile: usize) -> u128 {
            samples[(samples.len() * percentile).div_ceil(100).saturating_sub(1)]
        }

        let lanes = std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(REGIONS);
        #[cfg(target_os = "linux")]
        assert_benchmark_has_distinct_physical_cores(lanes);
        let (mut raw_ecs, raw_ecs_state) = raw_ecs();
        let (mut direct, direct_state) = direct(lanes);
        let (mut actor, actor_state) = actor(lanes);
        assert_eq!(raw_ecs_state, direct_state);
        assert_eq!(direct_state, actor_state);
        raw_ecs.sort_unstable();
        direct.sort_unstable();
        actor.sort_unstable();
        println!(
            "REGIONAL_COORDINATOR_ACTOR_OVERHEAD_BENCH entities={} regions={REGIONS} iterations={ITERATIONS} lanes={lanes} raw_ecs_p50_us={} raw_ecs_p95_us={} raw_ecs_p99_us={} direct_p50_us={} direct_p95_us={} direct_p99_us={} actor_p50_us={} actor_p95_us={} actor_p99_us={}",
            REGIONS * ENTITIES_PER_REGION,
            percentile(&raw_ecs, 50),
            percentile(&raw_ecs, 95),
            percentile(&raw_ecs, 99),
            percentile(&direct, 50),
            percentile(&direct, 95),
            percentile(&direct, 99),
            percentile(&actor, 50),
            percentile(&actor, 95),
            percentile(&actor, 99),
        );
    }

    #[test]
    #[ignore = "explicit debug cached animal mutation benchmark"]
    fn cached_animal_mutation_benchmark_report() {
        const ENTITIES: usize = 128;
        const ITERATIONS: usize = 200;

        fn run(force_actor: bool) -> (Vec<u128>, Vec<crate::EntitySnapshot>) {
            let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
                .expect("owner runtime");
            let handle = runtime.handle();
            let entities = (0..ENTITIES).map(|index| {
                let mut entity = cow(Vec3::new(
                    0.5 + (index % 16) as f64,
                    64.0,
                    0.5 + (index / 16) as f64,
                ));
                entity.animal = Some(AnimalBreedingState::adult());
                entity
            });
            let ids = handle
                .spawn_authoritative_batch(entities)
                .expect("benchmark animals")
                .into_iter()
                .collect::<HashSet<_>>();
            let mut samples = Vec::with_capacity(ITERATIONS);
            for iteration in 0..ITERATIONS {
                let states = handle
                    .snapshots_for_ids(&ids)
                    .expect("benchmark snapshots")
                    .into_iter()
                    .map(|snapshot| {
                        let mut animal = snapshot.animal.expect("animal state");
                        animal.love_ticks = if iteration.is_multiple_of(2) { 1 } else { 2 };
                        (snapshot, animal)
                    })
                    .collect::<Vec<_>>();
                if force_actor {
                    handle
                        .selected_read_routes
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clear();
                }
                let started = Instant::now();
                assert!(
                    handle
                        .set_animal_states_if_current(states)
                        .expect("animal mutation")
                );
                samples.push(started.elapsed().as_micros());
            }
            let snapshots = handle.snapshots().expect("final snapshots");
            drop(handle);
            runtime.shutdown().expect("runtime shutdown");
            (samples, snapshots)
        }

        fn run_concurrent(force_actor: bool) -> (Vec<u128>, Vec<crate::EntitySnapshot>) {
            let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
                .expect("owner runtime");
            let handle = runtime.handle();
            let entities = (0..ENTITIES).map(|index| {
                let region = index % 2;
                let local = index / 2;
                let mut entity = cow(Vec3::new(
                    region as f64 * 128.0 + 0.5 + (local % 8) as f64,
                    64.0,
                    0.5 + (local / 8) as f64,
                ));
                entity.animal = Some(AnimalBreedingState::adult());
                entity
            });
            let spawned = handle
                .spawn_authoritative_batch(entities)
                .expect("benchmark animals");
            let west = spawned.iter().step_by(2).copied().collect::<HashSet<_>>();
            let east = spawned
                .iter()
                .skip(1)
                .step_by(2)
                .copied()
                .collect::<HashSet<_>>();
            let mut samples = Vec::with_capacity(ITERATIONS);
            for iteration in 0..ITERATIONS {
                let build_states = |ids: &HashSet<EntityId>| {
                    handle
                        .snapshots_for_ids(ids)
                        .expect("benchmark snapshots")
                        .into_iter()
                        .map(|snapshot| {
                            let mut animal = snapshot.animal.expect("animal state");
                            animal.love_ticks = if iteration.is_multiple_of(2) { 1 } else { 2 };
                            (snapshot, animal)
                        })
                        .collect::<Vec<_>>()
                };
                let west_states = build_states(&west);
                let east_states = build_states(&east);
                if force_actor {
                    handle
                        .selected_read_routes
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clear();
                }
                let barrier = Arc::new(std::sync::Barrier::new(3));
                let started = Instant::now();
                std::thread::scope(|scope| {
                    let west_barrier = Arc::clone(&barrier);
                    let west_handle = &handle;
                    let west = scope.spawn(move || {
                        west_barrier.wait();
                        west_handle.set_animal_states_if_current(west_states)
                    });
                    let east_barrier = Arc::clone(&barrier);
                    let east_handle = &handle;
                    let east = scope.spawn(move || {
                        east_barrier.wait();
                        east_handle.set_animal_states_if_current(east_states)
                    });
                    barrier.wait();
                    assert!(west.join().expect("west worker").expect("west mutation"));
                    assert!(east.join().expect("east worker").expect("east mutation"));
                });
                samples.push(started.elapsed().as_micros());
            }
            let snapshots = handle.snapshots().expect("final snapshots");
            drop(handle);
            runtime.shutdown().expect("runtime shutdown");
            (samples, snapshots)
        }

        fn percentile(samples: &[u128], percentile: usize) -> u128 {
            samples[(samples.len() * percentile).div_ceil(100).saturating_sub(1)]
        }

        let (mut direct, direct_state) = run(false);
        let (mut actor, actor_state) = run(true);
        let (mut concurrent_direct, concurrent_direct_state) = run_concurrent(false);
        let (mut concurrent_actor, concurrent_actor_state) = run_concurrent(true);
        assert_eq!(direct_state, actor_state);
        assert_eq!(concurrent_direct_state, concurrent_actor_state);
        direct.sort_unstable();
        actor.sort_unstable();
        concurrent_direct.sort_unstable();
        concurrent_actor.sort_unstable();
        println!(
            "CACHED_ANIMAL_MUTATION_BENCH entities={ENTITIES} iterations={ITERATIONS} direct_p50_us={} direct_p95_us={} direct_p99_us={} actor_p50_us={} actor_p95_us={} actor_p99_us={} concurrent_direct_p50_us={} concurrent_direct_p95_us={} concurrent_direct_p99_us={} concurrent_actor_p50_us={} concurrent_actor_p95_us={} concurrent_actor_p99_us={}",
            percentile(&direct, 50),
            percentile(&direct, 95),
            percentile(&direct, 99),
            percentile(&actor, 50),
            percentile(&actor, 95),
            percentile(&actor, 99),
            percentile(&concurrent_direct, 50),
            percentile(&concurrent_direct, 95),
            percentile(&concurrent_direct, 99),
            percentile(&concurrent_actor, 50),
            percentile(&concurrent_actor, 95),
            percentile(&concurrent_actor, 99),
        );
    }

    #[test]
    fn owner_selected_reads_reject_stale_coordinator_indexes() {
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
                .expect("owner coordinator");
        let entity = coordinator
            .spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("cow");
        coordinator.uuids.clear();

        assert_eq!(
            coordinator.snapshots_for_ids(&HashSet::from([entity])),
            Err(super::RegionOwnerLaneError::InvalidMutation)
        );

        coordinator.shutdown().expect("coordinator shutdown");
    }

    #[test]
    fn owner_runtime_aggregates_one_shadow_comparison_across_lanes() {
        let mut store = RegionalEntityStore::new();
        store
            .assign_region(RegionKey::new(-1, 0), 0)
            .expect("west region");
        store
            .assign_region(RegionKey::new(1, 0), 1)
            .expect("east region");
        spawn_shadowed_legacy(
            &mut store,
            RegionKey::new(-1, 0),
            cow(Vec3::new(-0.5, 64.0, 0.5)),
        );
        spawn_shadowed_legacy(
            &mut store,
            RegionKey::new(1, 0),
            cow(Vec3::new(128.5, 64.0, 0.5)),
        );
        let runtime = super::RegionalOwnerRuntime::from_store(store, 2).expect("owner runtime");
        let handle = runtime.handle();

        let comparison = handle
            .compare_shadow(9, ShadowStage::PhysicsApply)
            .expect("owner shadow command")
            .expect("matching owner shadows");
        assert_eq!(comparison.compared_entities, 2);
        let status = handle.status().expect("owner status");
        assert_eq!(status.shadow.comparisons, 1);
        assert_eq!(status.shadow.compared_entities, 2);
        assert_eq!(
            status.shadow.compared_events,
            comparison.compared_events as u64
        );
        assert!(status.shadow.first_divergence.is_none());

        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
    }

    #[test]
    fn production_authority_moves_vehicle_and_passenger_as_one_group() {
        let mut authority = RegionalEntityAuthority::default();
        let mut boat = SpawnEntity::vehicle(
            VehicleKind::Boat,
            0,
            "minecraft:oak_boat",
            Vec3::new(127.5, 64.0, 0.5),
        );
        boat.vehicle.as_mut().expect("boat state").passenger = Some(EntityId(2));
        let ids = authority.spawn_authoritative_batch([boat, cow(Vec3::new(127.75, 64.0, 0.5))]);
        assert_eq!(ids, vec![EntityId(1), EntityId(2)]);

        assert_eq!(
            authority.apply_kinematics([movement(ids[0], Vec3::new(128.25, 64.0, 0.5))]),
            1
        );

        let migrated_boat = authority.snapshot(ids[0]).expect("migrated boat");
        let migrated_passenger = authority.snapshot(ids[1]).expect("migrated passenger");
        assert_eq!(migrated_boat.position, Vec3::new(128.25, 64.0, 0.5));
        assert_eq!(
            migrated_boat.vehicle.and_then(|vehicle| vehicle.passenger),
            Some(ids[1])
        );
        assert_eq!(migrated_passenger.position, Vec3::new(128.5, 64.0, 0.5));
        assert_eq!(authority.region_len(RegionKey::new(0, 0)), 0);
        assert_eq!(authority.region_len(RegionKey::new(1, 0)), 2);

        assert!(authority.set_position(ids[0], Vec3::new(127.5, 64.0, 0.5)));
        assert!(authority.set_position(ids[0], Vec3::new(128.25, 64.0, 0.5)));
        assert_eq!(authority.region_len(RegionKey::new(0, 0)), 0);
        assert_eq!(authority.region_len(RegionKey::new(1, 0)), 2);
        assert_eq!(
            authority
                .snapshot(ids[0])
                .and_then(|snapshot| snapshot.vehicle)
                .and_then(|vehicle| vehicle.passenger),
            Some(ids[1])
        );
    }

    #[test]
    fn production_authority_ignores_stale_passenger_state_after_group_crossing() {
        let mut authority = RegionalEntityAuthority::default();
        let mut boat = SpawnEntity::vehicle(
            VehicleKind::Boat,
            0,
            "minecraft:oak_boat",
            Vec3::new(127.5, 64.0, 0.5),
        );
        boat.vehicle.as_mut().expect("boat state").passenger = Some(EntityId(2));
        let ids = authority.spawn_authoritative_batch([boat, cow(Vec3::new(127.75, 64.0, 0.5))]);

        assert_eq!(
            authority.apply_kinematics([
                movement(ids[0], Vec3::new(128.25, 64.0, 0.5)),
                movement(ids[1], Vec3::new(127.75, 64.0, 0.5)),
            ]),
            2
        );

        assert_eq!(authority.region_len(RegionKey::new(0, 0)), 0);
        assert_eq!(authority.region_len(RegionKey::new(1, 0)), 2);
        assert_eq!(
            authority.snapshot(ids[1]).expect("passenger").position,
            Vec3::new(128.5, 64.0, 0.5)
        );
    }

    #[test]
    fn parallel_local_apply_keeps_vehicle_boundary_transfer_atomic() {
        let mut authority = RegionalEntityAuthority::default();
        let mut boat = SpawnEntity::vehicle(
            VehicleKind::Boat,
            0,
            "minecraft:oak_boat",
            Vec3::new(127.5, 64.0, 0.5),
        );
        boat.vehicle.as_mut().expect("boat state").passenger = Some(EntityId(2));
        let ids = authority.spawn_authoritative_batch([
            boat,
            cow(Vec3::new(127.75, 64.0, 0.5)),
            cow(Vec3::new(-0.5, 64.0, 0.5)),
            cow(Vec3::new(256.5, 64.0, 0.5)),
        ]);

        let applied = authority.apply_kinematics_parallel_authoritative(
            [
                movement(ids[0], Vec3::new(128.25, 64.0, 0.5)),
                movement(ids[1], Vec3::new(127.75, 64.0, 0.5)),
                movement(ids[2], Vec3::new(-0.25, 64.0, 0.5)),
                movement(ids[3], Vec3::new(256.75, 64.0, 0.5)),
            ],
            2,
        );
        assert_eq!(applied.len(), 4);
        assert_eq!(
            applied
                .iter()
                .find(|state| state.id == ids[1])
                .expect("authoritative passenger result")
                .position,
            Vec3::new(128.5, 64.0, 0.5)
        );

        assert_eq!(authority.region_len(RegionKey::new(0, 0)), 0);
        assert_eq!(authority.region_len(RegionKey::new(1, 0)), 2);
        assert_eq!(
            authority.snapshot(ids[1]).expect("passenger").position,
            Vec3::new(128.5, 64.0, 0.5)
        );
        assert_eq!(
            authority.snapshot(ids[2]).expect("west cow").position,
            Vec3::new(-0.25, 64.0, 0.5)
        );
        assert_eq!(
            authority.snapshot(ids[3]).expect("east cow").position,
            Vec3::new(256.75, 64.0, 0.5)
        );
    }

    #[test]
    fn production_authority_uses_vehicle_leader_when_passenger_crossing_arrives_first() {
        let mut authority = RegionalEntityAuthority::default();
        let mut boat = SpawnEntity::vehicle(
            VehicleKind::Boat,
            0,
            "minecraft:oak_boat",
            Vec3::new(127.5, 64.0, 0.5),
        );
        boat.vehicle.as_mut().expect("boat state").passenger = Some(EntityId(2));
        let ids = authority.spawn_authoritative_batch([boat, cow(Vec3::new(127.75, 64.0, 0.5))]);

        assert_eq!(
            authority.apply_kinematics([
                movement(ids[1], Vec3::new(129.75, 64.0, 0.5)),
                movement(ids[0], Vec3::new(128.25, 64.0, 0.5)),
            ]),
            2
        );

        assert_eq!(
            authority.snapshot(ids[0]).expect("boat").position,
            Vec3::new(128.25, 64.0, 0.5)
        );
        assert_eq!(
            authority.snapshot(ids[1]).expect("passenger").position,
            Vec3::new(128.5, 64.0, 0.5)
        );
    }

    #[test]
    fn production_authority_prioritizes_crossing_vehicle_when_passenger_has_lower_id() {
        let mut authority = RegionalEntityAuthority::default();
        let passenger = cow(Vec3::new(127.75, 64.0, 0.5));
        let mut boat = SpawnEntity::vehicle(
            VehicleKind::Boat,
            0,
            "minecraft:oak_boat",
            Vec3::new(127.5, 64.0, 0.5),
        );
        boat.vehicle.as_mut().expect("boat state").passenger = Some(EntityId(1));
        let ids = authority.spawn_authoritative_batch([passenger, boat]);

        assert_eq!(
            authority.apply_kinematics([
                movement(ids[0], Vec3::new(0.5, 64.0, 0.5)),
                movement(ids[1], Vec3::new(128.25, 64.0, 0.5)),
            ]),
            2
        );

        assert_eq!(
            authority.snapshot(ids[1]).expect("boat").position,
            Vec3::new(128.25, 64.0, 0.5)
        );
        assert_eq!(
            authority.snapshot(ids[0]).expect("passenger").position,
            Vec3::new(128.5, 64.0, 0.5)
        );
    }

    #[test]
    fn production_authority_rejects_vehicle_group_split_without_partial_move() {
        let mut authority = RegionalEntityAuthority::default();
        let mut boat = SpawnEntity::vehicle(
            VehicleKind::Boat,
            0,
            "minecraft:oak_boat",
            Vec3::new(127.5, 64.0, 0.5),
        );
        boat.vehicle.as_mut().expect("boat state").passenger = Some(EntityId(2));
        let ids = authority.spawn_authoritative_batch([boat, cow(Vec3::new(0.5, 64.0, 0.5))]);
        let before = authority.snapshots().collect::<Vec<_>>();

        assert_eq!(
            authority.apply_kinematics([movement(ids[0], Vec3::new(128.25, 64.0, 0.5))]),
            0
        );
        assert_eq!(authority.snapshots().collect::<Vec<_>>(), before);
        assert_eq!(authority.region_len(RegionKey::new(0, 0)), 2);
        assert_eq!(authority.region_len(RegionKey::new(1, 0)), 0);
    }

    #[test]
    fn production_authority_resolves_follow_target_across_region_boundary() {
        let mut authority = RegionalEntityAuthority::default();
        let target = cow(Vec3::new(127.5, 64.0, 0.5));
        let mut follower = cow(Vec3::new(126.5, 64.0, 0.5));
        follower.goal = GoalState::FollowTarget {
            target: EntityId(1),
            speed: 0.25,
        };
        let ids = authority.spawn_authoritative_batch([target, follower]);

        assert!(authority.set_position(ids[0], Vec3::new(128.5, 64.0, 0.5)));
        let active = HashSet::from([ids[1]]);
        let resolved = authority
            .prepare_goal_tick_with_pathing_for_ids(23, &active)
            .resolve(&WalkablePathing, PathingBudget::DEFAULT);
        let stats = authority.apply_prepared_goal_tick(resolved);

        assert_eq!(stats.missing_follow_targets, 0);
        assert_eq!(authority.region_len(RegionKey::new(0, 0)), 1);
        assert_eq!(authority.region_len(RegionKey::new(1, 0)), 1);
        let motion = authority.motion_state(ids[1]).expect("follower motion");
        assert!(motion.velocity.x > 0.0);
        assert_eq!(motion.velocity.z, 0.0);
        assert_eq!(
            authority.snapshot(ids[1]).expect("follower").goal,
            GoalState::FollowTarget {
                target: ids[0],
                speed: 0.25,
            }
        );
    }

    #[test]
    fn production_authority_rejects_follow_batch_when_local_target_migrates() {
        let mut authority = RegionalEntityAuthority::default();
        let target = cow(Vec3::new(127.5, 64.0, 0.5));
        let mut follower = cow(Vec3::new(126.5, 64.0, 0.5));
        follower.goal = GoalState::FollowTarget {
            target: EntityId(1),
            speed: 0.25,
        };
        let ids = authority.spawn_authoritative_batch([target, follower]);
        let active = HashSet::from([ids[1]]);
        let resolved = authority
            .prepare_goal_tick_with_pathing_for_ids(24, &active)
            .resolve(&WalkablePathing, PathingBudget::DEFAULT);

        assert!(authority.set_position(ids[0], Vec3::new(128.5, 64.0, 0.5)));
        let stats = authority.apply_prepared_goal_tick(resolved);

        assert_eq!(stats.decisions_applied, 0);
        assert_eq!(stats.missing_follow_targets, 0);
        assert_eq!(
            authority.motion_state(ids[1]).expect("follower").velocity,
            Vec3::ZERO
        );
    }

    #[test]
    fn production_authority_rejects_follow_batch_when_remote_target_moves() {
        let mut authority = RegionalEntityAuthority::default();
        let target = cow(Vec3::new(128.5, 64.0, 0.5));
        let mut follower = cow(Vec3::new(126.5, 64.0, 0.5));
        follower.goal = GoalState::FollowTarget {
            target: EntityId(1),
            speed: 0.25,
        };
        let ids = authority.spawn_authoritative_batch([target, follower]);
        let active = HashSet::from([ids[1]]);
        let resolved = authority
            .prepare_goal_tick_with_pathing_for_ids(25, &active)
            .resolve(&WalkablePathing, PathingBudget::DEFAULT);

        assert!(authority.set_position(ids[0], Vec3::new(129.5, 64.0, 0.5)));
        let stats = authority.apply_prepared_goal_tick(resolved);

        assert_eq!(stats.decisions_applied, 0);
        assert_eq!(stats.missing_follow_targets, 0);
        assert_eq!(
            authority.motion_state(ids[1]).expect("follower").velocity,
            Vec3::ZERO
        );
    }

    #[test]
    fn production_authority_rejects_follow_batch_when_follower_goal_changes() {
        let mut authority = RegionalEntityAuthority::default();
        let target = cow(Vec3::new(128.5, 64.0, 0.5));
        let mut follower = cow(Vec3::new(126.5, 64.0, 0.5));
        follower.goal = GoalState::FollowTarget {
            target: EntityId(1),
            speed: 0.25,
        };
        let ids = authority.spawn_authoritative_batch([target, follower]);
        let active = HashSet::from([ids[1]]);
        let resolved = authority
            .prepare_goal_tick_with_pathing_for_ids(26, &active)
            .resolve(&WalkablePathing, PathingBudget::DEFAULT);

        assert!(authority.set_goal(ids[1], GoalState::Idle));
        assert!(authority.set_velocity(ids[1], Vec3::new(0.4, 0.0, 0.0)));
        let stats = authority.apply_prepared_goal_tick(resolved);

        assert_eq!(stats.decisions_applied, 0);
        assert_eq!(
            authority.snapshot(ids[1]).expect("follower").goal,
            GoalState::Idle
        );
        assert_eq!(
            authority.motion_state(ids[1]).expect("follower").velocity,
            Vec3::new(0.4, 0.0, 0.0)
        );
    }

    #[test]
    fn production_authority_rejects_idle_batch_when_motion_changes() {
        let mut authority = RegionalEntityAuthority::default();
        let id = authority.spawn_authoritative(cow(Vec3::new(0.5, 64.0, 0.5)));
        assert!(authority.set_velocity(id, Vec3::new(0.25, 0.0, 0.0)));
        let resolved = authority
            .prepare_goal_tick_with_pathing_for_ids(27, &HashSet::from([id]))
            .resolve(&WalkablePathing, PathingBudget::DEFAULT);

        assert!(authority.set_velocity(id, Vec3::new(0.5, 0.0, 0.0)));
        let stats = authority.apply_prepared_goal_tick(resolved);

        assert_eq!(stats, GoalTickStats::default());
        assert_eq!(
            authority.motion_state(id).expect("idle cow").velocity,
            Vec3::new(0.5, 0.0, 0.0)
        );
    }

    #[test]
    fn parallel_goal_apply_validates_all_regions_before_mutation() {
        let mut authority = RegionalEntityAuthority::default();
        let ids = authority.spawn_authoritative_batch([
            cow(Vec3::new(-0.5, 64.0, 0.5)),
            cow(Vec3::new(128.5, 64.0, 0.5)),
        ]);
        assert!(authority.set_velocity(ids[0], Vec3::new(0.25, 0.0, 0.0)));
        assert!(authority.set_velocity(ids[1], Vec3::new(0.25, 0.0, 0.0)));
        let resolved = authority
            .prepare_goal_tick_with_pathing_for_ids(28, &ids.iter().copied().collect())
            .resolve(&WalkablePathing, PathingBudget::DEFAULT);

        assert!(authority.set_velocity(ids[0], Vec3::new(0.5, 0.0, 0.0)));
        let stats = authority.apply_prepared_goal_tick_parallel(resolved, 2);

        assert_eq!(stats, GoalTickStats::default());
        assert_eq!(
            authority.motion_state(ids[0]).expect("west cow").velocity,
            Vec3::new(0.5, 0.0, 0.0)
        );
        assert_eq!(
            authority.motion_state(ids[1]).expect("east cow").velocity,
            Vec3::new(0.25, 0.0, 0.0)
        );
    }

    #[test]
    fn production_authority_closes_phase_when_mutation_panics() {
        let mut authority = RegionalEntityAuthority::default();
        let duplicate_uuid = Uuid::from_u128(5001);
        let mut first = cow(Vec3::new(0.5, 64.0, 0.5));
        first.uuid = Some(duplicate_uuid);
        authority.spawn_authoritative(first);
        let mut duplicate = cow(Vec3::new(1.5, 64.0, 0.5));
        duplicate.uuid = Some(duplicate_uuid);

        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            authority.spawn_authoritative(duplicate)
        }));
        assert!(panic.is_err());

        let recovered = authority.spawn_authoritative(cow(Vec3::new(2.5, 64.0, 0.5)));
        assert!(authority.contains(recovered));
    }
}
