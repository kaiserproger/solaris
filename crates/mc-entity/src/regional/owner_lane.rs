use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashMap, HashSet};
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use super::tick::RegionalPhysicsWorld;
use super::{
    CompactEntityKinematicsFence, CompactEntityPhysicsFence, RegionKey, RegionLease, RegionPhase,
    RegionalEntityTickInput, RegionalGoalTickInputs, RegionalTickWorld, add_goal_tick_stats,
    goal_reference, order_vehicle_group_for_removal, snapshot_vehicle_reference,
};

use crate::lock_policy::lock_authoritative_mutex;
use crate::{
    AnimalBreedingState, EntityDamageRequest, EntityEffectRequest, EntityEffectResult,
    EntityGoalCheckpoint, EntityId, EntityItemStack, EntityKinematics, EntityMotionState,
    EntityPhysicsQuery, EntityPhysicsStep, EntitySimulationProjection, EntitySimulationResult,
    EntitySnapshot, EntityStore, EntityTrackingMotion, EntityVillagerGoalUpdate, GoalState,
    GoalTickStats, PreparedGoalTick, ResolvedGoalTick, Vec3,
};

mod entity_projections;
mod physics;

#[derive(Debug, Clone, Copy)]
pub(super) struct FencedKinematicsCandidate {
    pub lease: RegionLease,
    pub id: EntityId,
    pub uuid: uuid::Uuid,
    pub lifecycle: crate::EntityLifecycle,
    pub pickup_claimed: bool,
    pub vehicle_attached: bool,
    pub result: Option<EntitySimulationResult>,
}

impl FencedKinematicsCandidate {
    pub(super) fn eligible(self) -> bool {
        self.lifecycle == crate::EntityLifecycle::Alive
            && !self.pickup_claimed
            && !self.vehicle_attached
            && self
                .result
                .is_some_and(|result| result.physics.id == self.id)
    }

    pub(super) fn expected(self) -> Option<CompactEntityKinematicsFence> {
        let result = self.result?;
        Some(CompactEntityKinematicsFence {
            id: self.id,
            uuid: self.uuid,
            lifecycle: self.lifecycle,
            expected: EntityKinematics {
                id: self.id,
                position: result.physics.position,
                rotation: result.rotation,
                velocity: result.physics.velocity,
                on_ground: result.physics.on_ground,
            },
            pickup_claimed: self.pickup_claimed,
            vehicle_attached: self.vehicle_attached,
        })
    }
}

#[derive(Debug, Clone)]
pub(super) struct FencedKinematicsRead {
    pub lane: usize,
    pub candidates: Vec<FencedKinematicsCandidate>,
    pub motion_states: Vec<EntityMotionState>,
    pub state_version: u64,
}

fn fenced_kinematics_candidates(
    store: &EntityStore,
    lease: RegionLease,
    entities: &[EntityId],
    include_motion_states: bool,
) -> (Vec<FencedKinematicsCandidate>, Vec<EntityMotionState>) {
    let mut candidates = Vec::with_capacity(entities.len());
    let mut motion_states = Vec::with_capacity(if include_motion_states {
        entities.len()
    } else {
        0
    });
    store.visit_simulation_fence_results_for_ordered_ids(entities, |state, result| {
        if state.lifecycle != crate::EntityLifecycle::Alive {
            return;
        }
        let motion = state.motion;
        candidates.push(FencedKinematicsCandidate {
            lease,
            id: motion.id,
            uuid: state.uuid,
            lifecycle: state.lifecycle,
            pickup_claimed: state.pickup_claimed,
            vehicle_attached: state.vehicle_attached,
            result,
        });
        if include_motion_states {
            motion_states.push(motion);
        }
    });
    (candidates, motion_states)
}

fn fenced_kinematics_candidates_from_capture(
    lease: RegionLease,
    ids: &[EntityId],
    captured: Vec<crate::runtime::GoalSimulationCandidate>,
) -> Option<Vec<FencedKinematicsCandidate>> {
    if ids.len() != captured.len()
        || ids
            .iter()
            .zip(&captured)
            .any(|(&id, candidate)| id != candidate.id)
    {
        return None;
    }
    Some(
        captured
            .into_iter()
            .map(|candidate| FencedKinematicsCandidate {
                lease,
                id: candidate.id,
                uuid: candidate.uuid,
                lifecycle: candidate.lifecycle,
                pickup_claimed: candidate.pickup_claimed,
                vehicle_attached: candidate.vehicle_attached,
                result: candidate.result,
            })
            .collect(),
    )
}

pub(super) fn merge_fenced_kinematics_candidate_runs(
    runs: Vec<Vec<FencedKinematicsCandidate>>,
) -> Vec<FencedKinematicsCandidate> {
    let total = runs.iter().map(Vec::len).sum();
    let mut runs = runs
        .into_iter()
        .filter(|run| !run.is_empty())
        .collect::<Vec<_>>();
    runs.sort_unstable_by_key(|run| run[0].id);
    if runs
        .windows(2)
        .all(|pair| pair[0].last().expect("nonempty run").id < pair[1][0].id)
    {
        let mut merged = Vec::with_capacity(total);
        for run in runs {
            merged.extend(run);
        }
        return merged;
    }
    let mut runs = runs.into_iter().map(Vec::into_iter).collect::<Vec<_>>();
    let mut heads = runs.iter_mut().map(Iterator::next).collect::<Vec<_>>();
    let mut ready = BinaryHeap::new();
    for (run, candidate) in heads.iter().enumerate() {
        if let Some(candidate) = candidate {
            ready.push(Reverse((candidate.id, run)));
        }
    }

    let mut merged = Vec::with_capacity(total);
    while let Some(Reverse((_, run))) = ready.pop() {
        merged.push(heads[run].take().expect("queued candidate run head"));
        heads[run] = runs[run].next();
        if let Some(candidate) = &heads[run] {
            ready.push(Reverse((candidate.id, run)));
        }
    }
    merged
}

#[derive(Debug)]
pub(super) struct LocalKinematicsRegionBatch {
    pub lease: RegionLease,
    pub previous: Vec<CompactEntityKinematicsFence>,
    pub states: Vec<EntityKinematics>,
}

#[derive(Debug)]
pub(super) struct LocalKinematicsCommit {
    pub states: Vec<EntityKinematics>,
    pub state_version: u64,
}

pub(super) struct LocalPhysicsRegionBatch {
    pub lease: RegionLease,
    pub inputs: Vec<LocalPhysicsInput>,
    pub world_fence: Option<RegionalPhysicsWorld>,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct LocalPhysicsInput {
    pub previous: CompactEntityPhysicsFence,
    pub expected: EntityPhysicsQuery,
    pub step: EntityPhysicsStep,
    pub publish: bool,
}

#[derive(Debug)]
pub(super) struct LocalPhysicsCommit {
    pub rejected: Vec<EntityId>,
    pub accepted: Vec<(RegionLease, EntityMotionState)>,
    pub states: Vec<EntityKinematics>,
    pub state_version: u64,
}
type LocalPhysicsApply = (
    Vec<EntityId>,
    Vec<(RegionLease, EntityMotionState)>,
    Vec<EntityKinematics>,
);

pub(super) struct OwnerLanePhysicsTickInput {
    pub regions: Vec<RegionLease>,
    pub goal_motion: Vec<(RegionLease, EntityTrackingMotion)>,
    pub simulation_chunks: Arc<HashSet<(i32, i32)>>,
    pub world: Arc<RegionalTickWorld>,
}

pub(super) struct OwnerLaneTickOutput {
    pub goal_stats: GoalTickStats,
    pub active_entity_count: usize,
    pub active_hostile_ids: Vec<(EntityId, Vec3)>,
    pub villager_population_candidates: Vec<(EntityId, Vec3)>,
    pub villager_ids: Vec<(EntityId, Vec3)>,
    pub villager_proximity_seeds: Vec<(EntityId, Vec3)>,
    pub fallback_entity_ids: HashSet<EntityId>,
    pub goal_committed_motion: Vec<(RegionLease, EntityTrackingMotion)>,
    pub physics_regions: Vec<RegionLease>,
    pub resolved_direct_paths: HashSet<EntityId>,
    pub villager_profession_updates: Vec<EntitySnapshot>,
    pub mutated: bool,
}

pub(super) struct OwnerLanePhysicsOutput {
    pub fallback_candidates: Vec<FencedKinematicsCandidate>,
    pub committed_motion: Vec<(RegionLease, EntityTrackingMotion)>,
    pub terrain_pathing_additions: HashSet<EntityId>,
    pub physics_mutated: bool,
    pub state_version: u64,
    /// Worker-thread execution of this physics message (dispatch receipt to
    /// output assembly). The coordinator compares it against its nested
    /// completion wait to isolate worker message-queue delay.
    pub worker_exec_us: u64,
}

#[derive(Debug, Clone)]
pub(super) struct PreparedRegionGoalTick {
    pub goal_tick: PreparedGoalTick,
    pub checkpoints: Vec<EntityGoalCheckpoint>,
    pub goal_overrides: Vec<(EntityId, GoalState)>,
    pub pathing_aabbs: Vec<(EntityId, mc_physics::Aabb)>,
    pub snapshot_overrides: Vec<(EntitySnapshot, EntitySnapshot)>,
    pub villager_updates: Vec<EntityVillagerGoalUpdate>,
    pub cross_region_villager_candidates: Vec<EntityId>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RegionOwnerMutation {
    SetVelocity {
        entity: EntityId,
        velocity: Vec3,
    },
    SetAnimalState {
        entity: EntityId,
        animal: AnimalBreedingState,
    },
    SetAnimalStateIfCurrent {
        expected: Box<EntitySnapshot>,
        animal: AnimalBreedingState,
    },
    SetGrazingStateIfCurrent {
        expected: Box<EntitySnapshot>,
        velocity: Option<Vec3>,
        remaining_ticks: Option<u8>,
    },
    SetGoalIfCurrent {
        expected: Box<EntitySnapshot>,
        goal: GoalState,
    },
    SetItemStackIfCurrent {
        expected: Box<EntitySnapshot>,
        item_stack: Option<EntityItemStack>,
    },
    ReplaceSnapshotIfCurrent {
        expected: Box<EntitySnapshot>,
        next: Box<EntitySnapshot>,
        allow_type_change: bool,
    },
    SetKinematicsIfCurrent {
        expected: Box<EntitySnapshot>,
        state: EntityKinematics,
    },
    SetKinematicsBatchIfCurrent {
        expected: Vec<EntitySnapshot>,
        states: Vec<EntityKinematics>,
    },
    SetKinematicsBatchIfVersion {
        expected_state_version: u64,
        previous: Vec<CompactEntityKinematicsFence>,
        states: Vec<EntityKinematics>,
    },
    DamageIfCurrent {
        expected: Box<EntitySnapshot>,
        request: EntityDamageRequest,
    },
    ApplyEffectIfCurrent {
        expected: Box<EntitySnapshot>,
        request: Box<EntityEffectRequest>,
    },
    ApplyGoalBatch {
        expected: Vec<EntityGoalCheckpoint>,
        candidate_ids: Vec<EntityId>,
        expected_state_version: u64,
        resolved: Box<ResolvedGoalTick>,
        follow_targets: HashMap<EntityId, Vec3>,
        goal_overrides: Vec<(EntityId, GoalState)>,
        snapshot_overrides: Vec<(EntitySnapshot, EntitySnapshot)>,
        villager_updates: Vec<EntityVillagerGoalUpdate>,
    },
    InsertSnapshot(Box<EntitySnapshot>),
    InsertSnapshots(Vec<EntitySnapshot>),
    RemoveEntity(EntityId),
    RemoveIfCurrent(Box<EntitySnapshot>),
    RemoveSnapshotsIfCurrent(Vec<EntitySnapshot>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SequencedRegionMutation {
    pub sequence: u64,
    pub lease: RegionLease,
    pub mutation: RegionOwnerMutation,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegionOwnerBatch {
    pub phase: RegionPhase,
    pub sequence_watermark: u64,
    pub mutations: Vec<SequencedRegionMutation>,
}

#[derive(Debug, Clone)]
pub struct RegionOwnerCompletion {
    pub phase: RegionPhase,
    pub(super) goal_candidate_runs: Vec<Vec<FencedKinematicsCandidate>>,
    pub applied_sequences: Vec<u64>,
    pub goal_stats: GoalTickStats,
    pub effect_results: Vec<(u64, EntityEffectResult)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionOwnerLaneError {
    Closed,
    WorkerPanicked,
    SpawnFailed,
    InvalidLaneCount,
    InvalidQuery,
    BindingTokenCollision,
    BindingCapacityExceeded,
    EmptyStore,
    Busy,
    DuplicateRegion,
    WrongLane,
    StalePhase,
    StaleSequence,
    DuplicateSequence,
    UnknownRegion,
    StaleLease,
    UnknownEntity,
    InvalidMutation,
    Journal,
    OutcomeUnknown,
}

const LANE_WORKER_RUNNING: u8 = 0;
const LANE_WORKER_STOPPED: u8 = 1;
const LANE_WORKER_PANICKED: u8 = 2;

#[derive(Debug)]
struct RegionOwnerLaneHealth {
    state: AtomicU8,
}

impl RegionOwnerLaneHealth {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(LANE_WORKER_RUNNING),
        }
    }

    fn mark_stopped(&self) {
        self.state.store(LANE_WORKER_STOPPED, Ordering::Release);
    }

    fn mark_panicked(&self) {
        self.state.store(LANE_WORKER_PANICKED, Ordering::Release);
    }

    fn error(&self) -> Option<RegionOwnerLaneError> {
        match self.state.load(Ordering::Acquire) {
            LANE_WORKER_RUNNING => None,
            LANE_WORKER_PANICKED => Some(RegionOwnerLaneError::WorkerPanicked),
            _ => Some(RegionOwnerLaneError::Closed),
        }
    }

    fn error_after_disconnect(&self) -> RegionOwnerLaneError {
        loop {
            if let Some(error) = self.error() {
                return error;
            }
            std::thread::yield_now();
        }
    }
}

#[derive(Debug)]
pub struct RegionOwnerLaneStartError {
    pub error: RegionOwnerLaneError,
    pub regions: Vec<(RegionLease, EntityStore)>,
}

#[derive(Debug)]
pub(super) struct RegionOwnerInstallError {
    pub(super) error: RegionOwnerLaneError,
    pub(super) recovered: Option<Box<EntityStore>>,
}

enum RegionOwnerLaneMessage {
    InstallRegion {
        lease: RegionLease,
        store: Box<EntityStore>,
        reply:
            std::sync::mpsc::Sender<Result<RegionLease, (RegionOwnerLaneError, Box<EntityStore>)>>,
    },
    DetachRegion {
        lease: RegionLease,
        reply: std::sync::mpsc::Sender<Result<(RegionLease, EntityStore), RegionOwnerLaneError>>,
    },
    Prepare {
        batch: RegionOwnerBatch,
        reply: std::sync::mpsc::Sender<Result<RegionPhase, RegionOwnerLaneError>>,
    },
    PrepareAndCommit {
        batch: RegionOwnerBatch,
        reply: std::sync::mpsc::Sender<Result<RegionOwnerCompletion, RegionOwnerLaneError>>,
    },
    Commit {
        phase: RegionPhase,
        reply: std::sync::mpsc::Sender<Result<RegionOwnerCompletion, RegionOwnerLaneError>>,
    },
    Finalize {
        phase: RegionPhase,
        reply: std::sync::mpsc::Sender<Result<RegionPhase, RegionOwnerLaneError>>,
    },
    Rollback {
        phase: RegionPhase,
        reply: std::sync::mpsc::Sender<Result<RegionPhase, RegionOwnerLaneError>>,
    },
    Abort {
        phase: RegionPhase,
        reply: std::sync::mpsc::Sender<Result<RegionPhase, RegionOwnerLaneError>>,
    },
    Snapshot {
        lease: RegionLease,
        entity: EntityId,
        reply: std::sync::mpsc::Sender<Result<Option<EntitySnapshot>, RegionOwnerLaneError>>,
    },
    Snapshots {
        reply: std::sync::mpsc::Sender<Vec<EntitySnapshot>>,
    },
    SnapshotsForIds {
        entities: Vec<(RegionLease, EntityId)>,
        selection: super::SnapshotSelection,
        reply: std::sync::mpsc::Sender<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>>,
    },
    ExistingSnapshotsForIds {
        entities: Vec<(RegionLease, EntityId)>,
        reply: std::sync::mpsc::Sender<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>>,
    },
    SimulationProjectionsForIds {
        entities: Vec<(RegionLease, EntityId)>,
        reply:
            std::sync::mpsc::Sender<Result<Vec<EntitySimulationProjection>, RegionOwnerLaneError>>,
    },
    DespawnProjectionsForIds {
        entities: Vec<(RegionLease, EntityId)>,
        reply: std::sync::mpsc::Sender<
            Result<Vec<crate::EntityDespawnProjection>, RegionOwnerLaneError>,
        >,
    },
    GoalCheckpointsForIds {
        entities: Vec<(RegionLease, EntityId)>,
        reply: std::sync::mpsc::Sender<Result<Vec<EntityGoalCheckpoint>, RegionOwnerLaneError>>,
    },
    AliveKinematicsForIds {
        entities: Vec<(RegionLease, EntityId)>,
        reply: std::sync::mpsc::Sender<Result<FencedKinematicsRead, RegionOwnerLaneError>>,
    },
    ApplyLocalKinematicsIfVersion {
        batches: Vec<LocalKinematicsRegionBatch>,
        reply: std::sync::mpsc::Sender<Result<Option<LocalKinematicsCommit>, RegionOwnerLaneError>>,
    },
    ApplyLocalPhysicsIfCurrent {
        expected_state_version: u64,
        batches: Vec<LocalPhysicsRegionBatch>,
        reply: std::sync::mpsc::Sender<Result<LocalPhysicsCommit, RegionOwnerLaneError>>,
    },
    TickOwnedRegions {
        input: RegionalEntityTickInput,
        reply: std::sync::mpsc::Sender<Result<OwnerLaneTickOutput, RegionOwnerLaneError>>,
    },
    TickOwnedRegionPhysics {
        input: OwnerLanePhysicsTickInput,
        reply: std::sync::mpsc::Sender<Result<OwnerLanePhysicsOutput, RegionOwnerLaneError>>,
    },
    NearestVillager {
        leases: Vec<RegionLease>,
        center: Vec3,
        radius_squared: f64,
        excluded: HashSet<EntityId>,
        reply: std::sync::mpsc::Sender<Result<Option<EntitySnapshot>, RegionOwnerLaneError>>,
    },
    SaveBarrier {
        sequence_watermark: u64,
        leases: Vec<RegionLease>,
        reply: std::sync::mpsc::Sender<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>>,
    },
    PrepareGoalTick {
        lease: RegionLease,
        tick: u64,
        active_ids: HashSet<EntityId>,
        inputs: RegionalGoalTickInputs,
        reply: std::sync::mpsc::Sender<Result<PreparedRegionGoalTick, RegionOwnerLaneError>>,
    },
    #[cfg(test)]
    HoldForTest {
        entered: std::sync::mpsc::Sender<()>,
        release: Receiver<()>,
    },

    Shutdown {
        reply:
            std::sync::mpsc::Sender<Result<BTreeMap<RegionKey, EntityStore>, RegionOwnerLaneError>>,
    },
}

enum RegionOwnerUndo {
    Velocity {
        lease: RegionLease,
        entity: EntityId,
        velocity: Vec3,
    },
    AnimalState {
        lease: RegionLease,
        entity: EntityId,
        animal: AnimalBreedingState,
    },
    Goal {
        lease: RegionLease,
        snapshot: Box<EntitySnapshot>,
    },
    ItemStack {
        lease: RegionLease,
        entity: EntityId,
        item_stack: Option<EntityItemStack>,
    },
    Inserted {
        lease: RegionLease,
        entity: EntityId,
        previous_next_id: i32,
    },
    InsertedBatch {
        lease: RegionLease,
        entities: Vec<EntityId>,
        previous_next_id: i32,
    },
    Removed {
        lease: RegionLease,
        snapshot: Box<EntitySnapshot>,
    },
    RemovedBatch {
        lease: RegionLease,
        snapshots: Vec<EntitySnapshot>,
    },
    Kinematics {
        lease: RegionLease,
        state: EntityKinematics,
    },
    KinematicsBatch {
        lease: RegionLease,
        states: Vec<EntityKinematics>,
    },
    Damaged {
        lease: RegionLease,
        expected: Box<EntitySnapshot>,
    },
    Effect {
        lease: RegionLease,
        checkpoint: Box<crate::runtime::EntityEffectCheckpoint>,
    },
    Snapshot {
        lease: RegionLease,
        snapshot: Box<EntitySnapshot>,
        allow_type_change: bool,
    },
    GoalBatch {
        lease: RegionLease,
        checkpoints: Vec<EntityGoalCheckpoint>,
    },
}

struct CommittedRegionOwnerBatch {
    phase: RegionPhase,
    sequence_watermark: u64,
    undo: Vec<RegionOwnerUndo>,
}

pub struct RegionalOwnerLane {
    sender: SyncSender<RegionOwnerLaneMessage>,
    health: Arc<RegionOwnerLaneHealth>,
    admission: Arc<Mutex<()>>,
    state_version: Arc<AtomicU64>,
    worker: Option<JoinHandle<()>>,
    #[cfg(test)]
    panic_after_install: Arc<AtomicBool>,
    #[cfg(test)]
    prepare_requests: std::sync::Arc<AtomicU64>,
    #[cfg(test)]
    prepare_and_commit_requests: std::sync::Arc<AtomicU64>,
    #[cfg(test)]
    snapshot_batch_requests: std::sync::Arc<AtomicU64>,
    #[cfg(test)]
    goal_checkpoint_batch_requests: std::sync::Arc<AtomicU64>,
}

#[derive(Debug, Clone)]
pub(super) struct RegionalOwnerLaneReader {
    sender: SyncSender<RegionOwnerLaneMessage>,
    health: Arc<RegionOwnerLaneHealth>,
    admission: Arc<Mutex<()>>,
    state_version: Arc<AtomicU64>,
}

impl RegionalOwnerLaneReader {
    pub(super) fn unavailable_error(&self) -> RegionOwnerLaneError {
        self.health.error_after_disconnect()
    }

    pub(super) fn admission(&self) -> &Mutex<()> {
        &self.admission
    }

    pub(super) fn state_version(&self) -> u64 {
        self.state_version.load(Ordering::Acquire)
    }

    #[cfg(test)]
    pub(super) fn hold_for_test(
        &self,
        entered: std::sync::mpsc::Sender<()>,
        release: Receiver<()>,
    ) -> Result<(), RegionOwnerLaneError> {
        self.sender
            .send(RegionOwnerLaneMessage::HoldForTest { entered, release })
            .map_err(|_| self.unavailable_error())
    }
    pub(super) fn apply_local_physics_if_current(
        &self,
        expected_state_version: u64,
        batches: Vec<LocalPhysicsRegionBatch>,
    ) -> Result<Receiver<Result<LocalPhysicsCommit, RegionOwnerLaneError>>, RegionOwnerLaneError>
    {
        let (reply, committed) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::ApplyLocalPhysicsIfCurrent {
                expected_state_version,
                batches,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        Ok(committed)
    }

    pub(super) fn tick_owned_regions(
        &self,
        input: RegionalEntityTickInput,
    ) -> Result<Receiver<Result<OwnerLaneTickOutput, RegionOwnerLaneError>>, RegionOwnerLaneError>
    {
        let (reply, completed) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::TickOwnedRegions { input, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(completed)
    }

    pub(super) fn tick_owned_region_physics(
        &self,
        input: OwnerLanePhysicsTickInput,
    ) -> Result<Receiver<Result<OwnerLanePhysicsOutput, RegionOwnerLaneError>>, RegionOwnerLaneError>
    {
        let (reply, completed) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::TickOwnedRegionPhysics { input, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(completed)
    }

    pub(super) fn prepare_and_commit(
        &self,
        batch: RegionOwnerBatch,
    ) -> Result<Receiver<Result<RegionOwnerCompletion, RegionOwnerLaneError>>, RegionOwnerLaneError>
    {
        let (reply, committed) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::PrepareAndCommit { batch, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(committed)
    }

    pub(super) fn request_existing_snapshots_for_ids(
        &self,
        entities: Vec<(RegionLease, EntityId)>,
    ) -> Result<Receiver<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>>, RegionOwnerLaneError>
    {
        let (reply, snapshots) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::ExistingSnapshotsForIds { entities, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(snapshots)
    }

    pub(super) fn request_goal_checkpoints_for_ids(
        &self,
        entities: Vec<(RegionLease, EntityId)>,
    ) -> Result<
        Receiver<Result<Vec<EntityGoalCheckpoint>, RegionOwnerLaneError>>,
        RegionOwnerLaneError,
    > {
        let (reply, checkpoints) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::GoalCheckpointsForIds { entities, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(checkpoints)
    }

    pub(super) fn request_alive_kinematics_for_ids(
        &self,
        entities: Vec<(RegionLease, EntityId)>,
    ) -> Result<Receiver<Result<FencedKinematicsRead, RegionOwnerLaneError>>, RegionOwnerLaneError>
    {
        let (reply, states) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::AliveKinematicsForIds { entities, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(states)
    }

    #[cfg(test)]
    pub(super) fn request_nearest_villager(
        &self,
        leases: Vec<RegionLease>,
        center: Vec3,
        radius_squared: f64,
        excluded: HashSet<EntityId>,
    ) -> Result<Receiver<Result<Option<EntitySnapshot>, RegionOwnerLaneError>>, RegionOwnerLaneError>
    {
        let (reply, snapshot) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::NearestVillager {
                leases,
                center,
                radius_squared,
                excluded,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        Ok(snapshot)
    }

    pub(super) fn finalize(
        &self,
        phase: RegionPhase,
    ) -> Result<Receiver<Result<RegionPhase, RegionOwnerLaneError>>, RegionOwnerLaneError> {
        let (reply, finalized) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::Finalize { phase, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(finalized)
    }

    pub(super) fn rollback(
        &self,
        phase: RegionPhase,
    ) -> Result<Receiver<Result<RegionPhase, RegionOwnerLaneError>>, RegionOwnerLaneError> {
        let (reply, rolled_back) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::Rollback { phase, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(rolled_back)
    }

    pub(super) fn rollback_committed(
        &self,
        phase: RegionPhase,
    ) -> Result<(), RegionOwnerLaneError> {
        let rolled_back = self.rollback(phase)?;
        match rolled_back.recv().map_err(|_| self.unavailable_error())? {
            Ok(rolled_back) if rolled_back == phase => Ok(()),
            Ok(_) => Err(RegionOwnerLaneError::StalePhase),
            Err(error) => Err(error),
        }
    }

    pub(super) fn request_snapshots_for_ids(
        &self,
        entities: Vec<(RegionLease, EntityId)>,
    ) -> Result<Receiver<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>>, RegionOwnerLaneError>
    {
        let (reply, snapshots) = channel();
        match self
            .sender
            .try_send(RegionOwnerLaneMessage::SnapshotsForIds {
                entities,
                selection: super::SnapshotSelection::All,
                reply,
            }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => return Err(RegionOwnerLaneError::Busy),
            Err(TrySendError::Disconnected(_)) => return Err(self.unavailable_error()),
        }
        Ok(snapshots)
    }
}

impl RegionalOwnerLane {
    const QUEUE_CAPACITY: usize = 64;

    fn unavailable_error(&self) -> RegionOwnerLaneError {
        self.health.error_after_disconnect()
    }

    pub fn spawn(
        lane: usize,
        regions: impl IntoIterator<Item = (RegionLease, EntityStore)>,
    ) -> Result<Self, RegionOwnerLaneStartError> {
        Self::spawn_after(lane, regions, RegionPhase(0), 0)
    }

    pub(super) fn spawn_after(
        lane: usize,
        regions: impl IntoIterator<Item = (RegionLease, EntityStore)>,
        last_phase: RegionPhase,
        last_sequence: u64,
    ) -> Result<Self, RegionOwnerLaneStartError> {
        let regions = regions.into_iter().collect::<Vec<_>>();
        let mut keys = HashSet::with_capacity(regions.len());
        if regions.iter().any(|(lease, _)| lease.lane != lane) {
            return Err(RegionOwnerLaneStartError {
                error: RegionOwnerLaneError::WrongLane,
                regions,
            });
        }
        if regions.iter().any(|(lease, _)| !keys.insert(lease.key)) {
            return Err(RegionOwnerLaneStartError {
                error: RegionOwnerLaneError::DuplicateRegion,
                regions,
            });
        }
        let mut owned = BTreeMap::new();
        for (lease, store) in regions {
            owned.insert(lease.key, (lease, store));
        }
        let (sender, receiver) = sync_channel(Self::QUEUE_CAPACITY);
        let health = Arc::new(RegionOwnerLaneHealth::new());
        let worker_health = Arc::clone(&health);
        let admission = Arc::new(Mutex::new(()));
        let state_version = Arc::new(AtomicU64::new(0));
        let worker_state_version = Arc::clone(&state_version);
        #[cfg(test)]
        let panic_after_install = Arc::new(AtomicBool::new(false));
        #[cfg(test)]
        let worker_panic_after_install = Arc::clone(&panic_after_install);
        #[cfg(test)]
        let prepare_requests = std::sync::Arc::new(AtomicU64::new(0));
        #[cfg(test)]
        let prepare_and_commit_requests = std::sync::Arc::new(AtomicU64::new(0));
        #[cfg(test)]
        let snapshot_batch_requests = std::sync::Arc::new(AtomicU64::new(0));
        #[cfg(test)]
        let goal_checkpoint_batch_requests = std::sync::Arc::new(AtomicU64::new(0));
        let handoff = std::sync::Arc::new(std::sync::Mutex::new(Some(owned)));
        let worker_handoff = std::sync::Arc::clone(&handoff);
        let worker = match std::thread::Builder::new()
            .name(format!("solaris-region-owner-{lane}"))
            .spawn(move || {
                let owned =
                    lock_authoritative_mutex(&worker_handoff, "regional.owner_lane_start_handoff")
                        .take()
                        .expect("owner lane startup handoff remains available");
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_region_owner_lane(
                        lane,
                        owned,
                        last_phase.0,
                        last_sequence,
                        &receiver,
                        worker_state_version,
                        #[cfg(test)]
                        worker_panic_after_install,
                    );
                }));
                if outcome.is_err() {
                    worker_health.mark_panicked();
                } else {
                    worker_health.mark_stopped();
                }
            }) {
            Ok(worker) => worker,
            Err(_) => {
                let owned = lock_authoritative_mutex(&handoff, "regional.owner_lane_start_handoff")
                    .take()
                    .expect("failed spawn keeps owner stores in handoff");
                return Err(RegionOwnerLaneStartError {
                    error: RegionOwnerLaneError::SpawnFailed,
                    regions: owned.into_values().collect(),
                });
            }
        };
        Ok(Self {
            sender,
            health,
            admission,
            state_version,
            worker: Some(worker),
            #[cfg(test)]
            panic_after_install,
            #[cfg(test)]
            prepare_requests,
            #[cfg(test)]
            prepare_and_commit_requests,
            #[cfg(test)]
            snapshot_batch_requests,
            #[cfg(test)]
            goal_checkpoint_batch_requests,
        })
    }

    pub fn prepare(
        &self,
        batch: RegionOwnerBatch,
    ) -> Result<Receiver<Result<RegionPhase, RegionOwnerLaneError>>, RegionOwnerLaneError> {
        #[cfg(test)]
        self.prepare_requests.fetch_add(1, Ordering::Relaxed);
        let (reply, prepared) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::Prepare { batch, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(prepared)
    }

    pub(super) fn prepare_and_commit(
        &self,
        batch: RegionOwnerBatch,
    ) -> Result<Receiver<Result<RegionOwnerCompletion, RegionOwnerLaneError>>, RegionOwnerLaneError>
    {
        #[cfg(test)]
        {
            self.prepare_requests.fetch_add(1, Ordering::Relaxed);
            self.prepare_and_commit_requests
                .fetch_add(1, Ordering::Relaxed);
        }
        let (reply, committed) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::PrepareAndCommit { batch, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(committed)
    }

    pub(super) fn apply_local_kinematics_if_version(
        &self,
        batches: Vec<LocalKinematicsRegionBatch>,
    ) -> Result<
        Receiver<Result<Option<LocalKinematicsCommit>, RegionOwnerLaneError>>,
        RegionOwnerLaneError,
    > {
        let (reply, committed) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::ApplyLocalKinematicsIfVersion { batches, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(committed)
    }

    #[cfg(test)]
    pub(super) fn panic_after_next_install_for_test(&self) {
        self.panic_after_install.store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(super) fn prepare_request_count(&self) -> u64 {
        self.prepare_requests.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(super) fn prepare_and_commit_request_count(&self) -> u64 {
        self.prepare_and_commit_requests.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(super) fn reset_snapshot_batch_request_count(&self) {
        self.snapshot_batch_requests.store(0, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(super) fn snapshot_batch_request_count(&self) -> u64 {
        self.snapshot_batch_requests.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(super) fn reset_goal_checkpoint_batch_request_count(&self) {
        self.goal_checkpoint_batch_requests
            .store(0, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(super) fn goal_checkpoint_batch_request_count(&self) -> u64 {
        self.goal_checkpoint_batch_requests.load(Ordering::Relaxed)
    }

    pub(super) fn reader(&self) -> RegionalOwnerLaneReader {
        RegionalOwnerLaneReader {
            sender: self.sender.clone(),
            health: Arc::clone(&self.health),
            admission: Arc::clone(&self.admission),
            state_version: Arc::clone(&self.state_version),
        }
    }

    pub(super) fn install_region(
        &self,
        lease: RegionLease,
        store: EntityStore,
    ) -> Result<(), RegionOwnerInstallError> {
        let (reply, installed) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::InstallRegion {
                lease,
                store: Box::new(store),
                reply,
            })
            .map_err(|error| match error.0 {
                RegionOwnerLaneMessage::InstallRegion { store, .. } => RegionOwnerInstallError {
                    error: self.unavailable_error(),
                    recovered: Some(store),
                },
                _ => unreachable!("send returned the install message"),
            })?;
        match installed.recv() {
            Ok(Ok(installed)) if installed == lease => Ok(()),
            Ok(Ok(_)) => unreachable!("owner lane echoes the requested lease"),
            Ok(Err((error, store))) => Err(RegionOwnerInstallError {
                error,
                recovered: Some(store),
            }),
            Err(_) => Err(RegionOwnerInstallError {
                error: self.unavailable_error(),
                recovered: None,
            }),
        }
    }

    pub(super) fn detach_region(
        &self,
        lease: RegionLease,
    ) -> Result<(RegionLease, EntityStore), RegionOwnerLaneError> {
        let (reply, detached) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::DetachRegion { lease, reply })
            .map_err(|_| self.unavailable_error())?;
        detached.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn commit(
        &self,
        phase: RegionPhase,
    ) -> Result<Receiver<Result<RegionOwnerCompletion, RegionOwnerLaneError>>, RegionOwnerLaneError>
    {
        let (reply, committed) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::Commit { phase, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(committed)
    }

    pub fn abort(
        &self,
        phase: RegionPhase,
    ) -> Result<Receiver<Result<RegionPhase, RegionOwnerLaneError>>, RegionOwnerLaneError> {
        let (reply, aborted) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::Abort { phase, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(aborted)
    }

    pub fn finalize(
        &self,
        phase: RegionPhase,
    ) -> Result<Receiver<Result<RegionPhase, RegionOwnerLaneError>>, RegionOwnerLaneError> {
        let (reply, finalized) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::Finalize { phase, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(finalized)
    }

    pub fn rollback(
        &self,
        phase: RegionPhase,
    ) -> Result<Receiver<Result<RegionPhase, RegionOwnerLaneError>>, RegionOwnerLaneError> {
        let (reply, rolled_back) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::Rollback { phase, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(rolled_back)
    }

    pub(super) fn snapshot(
        &self,
        lease: RegionLease,
        entity: EntityId,
    ) -> Result<Option<EntitySnapshot>, RegionOwnerLaneError> {
        let (reply, snapshot) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::Snapshot {
                lease,
                entity,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        snapshot.recv().map_err(|_| self.unavailable_error())?
    }

    pub(super) fn request_snapshots(
        &self,
    ) -> Result<Receiver<Vec<EntitySnapshot>>, RegionOwnerLaneError> {
        #[cfg(test)]
        self.snapshot_batch_requests.fetch_add(1, Ordering::Relaxed);
        let (reply, snapshots) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::Snapshots { reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(snapshots)
    }

    pub(super) fn request_snapshots_for_ids(
        &self,
        entities: Vec<(RegionLease, EntityId)>,
        selection: super::SnapshotSelection,
    ) -> Result<Receiver<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>>, RegionOwnerLaneError>
    {
        #[cfg(test)]
        self.snapshot_batch_requests.fetch_add(1, Ordering::Relaxed);
        let (reply, snapshots) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::SnapshotsForIds {
                entities,
                selection,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        Ok(snapshots)
    }

    pub(super) fn request_goal_checkpoints_for_ids_fenced(
        &self,
        entities: Vec<(RegionLease, EntityId)>,
    ) -> Result<
        Receiver<Result<Vec<EntityGoalCheckpoint>, RegionOwnerLaneError>>,
        RegionOwnerLaneError,
    > {
        #[cfg(test)]
        self.goal_checkpoint_batch_requests
            .fetch_add(1, Ordering::Relaxed);
        self.reader().request_goal_checkpoints_for_ids(entities)
    }

    pub(super) fn request_goal_checkpoints_for_ids_admitted(
        &self,
        entities: Vec<(RegionLease, EntityId)>,
    ) -> Result<
        Receiver<Result<Vec<EntityGoalCheckpoint>, RegionOwnerLaneError>>,
        RegionOwnerLaneError,
    > {
        #[cfg(test)]
        self.goal_checkpoint_batch_requests
            .fetch_add(1, Ordering::Relaxed);
        let _admission = lock_authoritative_mutex(&self.admission, "regional.owner_lane_admission");
        self.reader().request_goal_checkpoints_for_ids(entities)
    }

    pub(super) fn request_existing_snapshots_for_ids(
        &self,
        entities: Vec<(RegionLease, EntityId)>,
    ) -> Result<Receiver<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>>, RegionOwnerLaneError>
    {
        let (reply, snapshots) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::ExistingSnapshotsForIds { entities, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(snapshots)
    }

    pub(super) fn request_nearest_villager(
        &self,
        leases: Vec<RegionLease>,
        center: Vec3,
        radius_squared: f64,
        excluded: HashSet<EntityId>,
    ) -> Result<Receiver<Result<Option<EntitySnapshot>, RegionOwnerLaneError>>, RegionOwnerLaneError>
    {
        let (reply, snapshot) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::NearestVillager {
                leases,
                center,
                radius_squared,
                excluded,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        Ok(snapshot)
    }

    pub(super) fn request_save_barrier(
        &self,
        sequence_watermark: u64,
        leases: Vec<RegionLease>,
    ) -> Result<Receiver<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>>, RegionOwnerLaneError>
    {
        let (reply, snapshots) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::SaveBarrier {
                sequence_watermark,
                leases,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        Ok(snapshots)
    }

    pub(super) fn request_goal_tick(
        &self,
        lease: RegionLease,
        tick: u64,
        active_ids: HashSet<EntityId>,
        inputs: RegionalGoalTickInputs,
    ) -> Result<Receiver<Result<PreparedRegionGoalTick, RegionOwnerLaneError>>, RegionOwnerLaneError>
    {
        let (reply, prepared) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::PrepareGoalTick {
                lease,
                tick,
                active_ids,
                inputs,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        Ok(prepared)
    }

    pub(super) fn request_goal_tick_admitted(
        &self,
        lease: RegionLease,
        tick: u64,
        active_ids: HashSet<EntityId>,
        inputs: RegionalGoalTickInputs,
    ) -> Result<
        (
            u64,
            Receiver<Result<PreparedRegionGoalTick, RegionOwnerLaneError>>,
        ),
        RegionOwnerLaneError,
    > {
        let _admission = lock_authoritative_mutex(&self.admission, "regional.owner_lane_admission");
        let state_version = self.state_version.load(Ordering::Acquire);
        let completion = self.request_goal_tick(lease, tick, active_ids, inputs)?;
        Ok((state_version, completion))
    }

    pub fn shutdown(mut self) -> Result<BTreeMap<RegionKey, EntityStore>, RegionOwnerLaneError> {
        self.stop()
    }

    fn stop(&mut self) -> Result<BTreeMap<RegionKey, EntityStore>, RegionOwnerLaneError> {
        let Some(worker) = self.worker.take() else {
            return Err(self.unavailable_error());
        };
        let (reply, stores) = channel();
        if self
            .sender
            .send(RegionOwnerLaneMessage::Shutdown { reply })
            .is_err()
        {
            let joined = worker.join();
            return if joined.is_err()
                || self.health.error() == Some(RegionOwnerLaneError::WorkerPanicked)
            {
                Err(RegionOwnerLaneError::WorkerPanicked)
            } else {
                Err(self.unavailable_error())
            };
        }
        let stores = stores.recv();
        let joined = worker.join();
        if joined.is_err() || self.health.error() == Some(RegionOwnerLaneError::WorkerPanicked) {
            return Err(RegionOwnerLaneError::WorkerPanicked);
        }
        stores.map_err(|_| self.unavailable_error())?
    }
}

impl Drop for RegionalOwnerLane {
    fn drop(&mut self) {
        if self.worker.is_some() {
            let _ = self.stop();
        }
    }
}

fn run_region_owner_lane(
    lane: usize,
    mut regions: BTreeMap<RegionKey, (RegionLease, EntityStore)>,
    mut last_phase: u64,
    mut last_sequence: u64,
    receiver: &Receiver<RegionOwnerLaneMessage>,
    state_version: Arc<AtomicU64>,
    #[cfg(test)] panic_after_install: Arc<AtomicBool>,
) {
    let mut pending = None;
    let mut committed = None;
    while let Ok(message) = receiver.recv() {
        match message {
            RegionOwnerLaneMessage::InstallRegion {
                lease,
                store,
                reply,
            } => {
                let result = if pending.is_some() || committed.is_some() {
                    Err((RegionOwnerLaneError::Busy, store))
                } else if lease.lane != lane {
                    Err((RegionOwnerLaneError::WrongLane, store))
                } else {
                    match regions.entry(lease.key) {
                        std::collections::btree_map::Entry::Vacant(entry) => {
                            entry.insert((lease, *store));
                            Ok(lease)
                        }
                        std::collections::btree_map::Entry::Occupied(_) => {
                            Err((RegionOwnerLaneError::DuplicateRegion, store))
                        }
                    }
                };
                if result.is_ok() {
                    state_version.fetch_add(1, Ordering::Release);
                    #[cfg(test)]
                    if panic_after_install.swap(false, Ordering::AcqRel) {
                        panic!("injected owner lane panic after region store installation");
                    }
                }
                let _ = reply.send(result);
            }
            RegionOwnerLaneMessage::DetachRegion { lease, reply } => {
                let result = if pending.is_some() || committed.is_some() {
                    Err(RegionOwnerLaneError::Busy)
                } else if lease.lane != lane {
                    Err(RegionOwnerLaneError::WrongLane)
                } else if regions
                    .get(&lease.key)
                    .is_some_and(|(current, _)| *current != lease)
                {
                    Err(RegionOwnerLaneError::StaleLease)
                } else {
                    regions
                        .remove(&lease.key)
                        .ok_or(RegionOwnerLaneError::UnknownRegion)
                };
                if result.is_ok() {
                    state_version.fetch_add(1, Ordering::Release);
                }
                let _ = reply.send(result);
            }
            RegionOwnerLaneMessage::Prepare { batch, reply } => {
                let result = if pending.is_some() || committed.is_some() {
                    Err(RegionOwnerLaneError::Busy)
                } else {
                    prepare_region_owner_batch(
                        lane,
                        &regions,
                        last_phase,
                        last_sequence,
                        state_version.load(Ordering::Acquire),
                        batch,
                    )
                    .map(|batch| {
                        let phase = batch.phase;
                        last_phase = phase.0;
                        pending = Some(batch);
                        phase
                    })
                };
                if result.is_ok() {
                    state_version.fetch_add(1, Ordering::Release);
                }
                let _ = reply.send(result);
            }
            RegionOwnerLaneMessage::PrepareAndCommit { batch, reply } => {
                let result = if pending.is_some() || committed.is_some() {
                    Err(RegionOwnerLaneError::Busy)
                } else {
                    prepare_region_owner_batch(
                        lane,
                        &regions,
                        last_phase,
                        last_sequence,
                        state_version.load(Ordering::Acquire),
                        batch,
                    )
                    .and_then(|batch| {
                        let phase = batch.phase;
                        last_phase = phase.0;
                        apply_prepared_region_owner_batch(&mut regions, batch).map(
                            |(completion, applied)| {
                                committed = Some(applied);
                                completion
                            },
                        )
                    })
                };
                if result.is_ok() {
                    state_version.fetch_add(1, Ordering::Release);
                }
                let _ = reply.send(result);
            }
            RegionOwnerLaneMessage::Commit { phase, reply } => {
                let result = match pending.take() {
                    Some(batch) if batch.phase == phase => {
                        match apply_prepared_region_owner_batch(&mut regions, batch) {
                            Ok((completion, applied)) => {
                                committed = Some(applied);
                                Ok(completion)
                            }
                            Err(error) => Err(error),
                        }
                    }
                    Some(batch) => {
                        pending = Some(batch);
                        Err(RegionOwnerLaneError::StalePhase)
                    }
                    None => Err(RegionOwnerLaneError::StalePhase),
                };
                if result.is_ok() {
                    state_version.fetch_add(1, Ordering::Release);
                }
                let _ = reply.send(result);
            }
            RegionOwnerLaneMessage::Finalize { phase, reply } => {
                let result = match committed.take() {
                    Some(applied) if applied.phase == phase => {
                        last_sequence = applied.sequence_watermark;
                        Ok(phase)
                    }
                    Some(applied) => {
                        committed = Some(applied);
                        Err(RegionOwnerLaneError::StalePhase)
                    }
                    None => Err(RegionOwnerLaneError::StalePhase),
                };
                if result.is_ok() {
                    state_version.fetch_add(1, Ordering::Release);
                }
                let _ = reply.send(result);
            }
            RegionOwnerLaneMessage::Rollback { phase, reply } => {
                let result = match committed.take() {
                    Some(applied) if applied.phase == phase => {
                        rollback_region_owner_batch(&mut regions, applied).map(|()| phase)
                    }
                    Some(applied) => {
                        committed = Some(applied);
                        Err(RegionOwnerLaneError::StalePhase)
                    }
                    None => Err(RegionOwnerLaneError::StalePhase),
                };
                if result.is_ok() {
                    state_version.fetch_add(1, Ordering::Release);
                }
                let _ = reply.send(result);
            }
            RegionOwnerLaneMessage::Abort { phase, reply } => {
                let result = match pending.take() {
                    Some(batch) if batch.phase == phase => Ok(phase),
                    Some(batch) => {
                        pending = Some(batch);
                        Err(RegionOwnerLaneError::StalePhase)
                    }
                    None => Err(RegionOwnerLaneError::StalePhase),
                };
                let _ = reply.send(result);
            }
            RegionOwnerLaneMessage::Snapshot {
                lease,
                entity,
                reply,
            } => {
                let result = if lease.lane != lane {
                    Err(RegionOwnerLaneError::WrongLane)
                } else if let Some((current, store)) = regions.get(&lease.key) {
                    if *current == lease {
                        Ok(store.snapshot(entity))
                    } else {
                        Err(RegionOwnerLaneError::StaleLease)
                    }
                } else {
                    Err(RegionOwnerLaneError::UnknownRegion)
                };
                let _ = reply.send(result);
            }
            RegionOwnerLaneMessage::Snapshots { reply } => {
                let mut snapshots = regions
                    .values()
                    .flat_map(|(_, store)| store.snapshots())
                    .collect::<Vec<_>>();
                snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
                let _ = reply.send(snapshots);
            }
            RegionOwnerLaneMessage::SnapshotsForIds {
                entities,
                selection,
                reply,
            } => {
                let mut snapshots = Vec::with_capacity(entities.len());
                let mut error = (pending.is_some() || committed.is_some())
                    .then_some(RegionOwnerLaneError::Busy);
                if error.is_none() {
                    for (lease, entity) in entities {
                        if lease.lane != lane {
                            error = Some(RegionOwnerLaneError::WrongLane);
                            break;
                        }
                        let Some((current, store)) = regions.get(&lease.key) else {
                            error = Some(RegionOwnerLaneError::UnknownRegion);
                            break;
                        };
                        if *current != lease {
                            error = Some(RegionOwnerLaneError::StaleLease);
                            break;
                        }
                        if let super::SnapshotSelection::SheepGrazing { include_idle } = &selection
                            && !include_idle.contains(&entity)
                        {
                            let Some(active) = store.sheep_grazing_activity(entity) else {
                                error = Some(RegionOwnerLaneError::UnknownEntity);
                                break;
                            };
                            if !active {
                                continue;
                            }
                        }
                        let Some(snapshot) = store.snapshot(entity) else {
                            error = Some(RegionOwnerLaneError::UnknownEntity);
                            break;
                        };
                        snapshots.push(snapshot);
                    }
                }
                snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
                let _ = reply.send(match error {
                    Some(error) => Err(error),
                    None => Ok(snapshots),
                });
            }
            RegionOwnerLaneMessage::ExistingSnapshotsForIds { entities, reply } => {
                let mut snapshots = Vec::with_capacity(entities.len());
                let mut error = None;
                for (lease, entity) in entities {
                    if lease.lane != lane {
                        error = Some(RegionOwnerLaneError::WrongLane);
                        break;
                    }
                    let Some((current, store)) = regions.get(&lease.key) else {
                        error = Some(RegionOwnerLaneError::UnknownRegion);
                        break;
                    };
                    if *current != lease {
                        error = Some(RegionOwnerLaneError::StaleLease);
                        break;
                    }
                    if let Some(snapshot) = store.snapshot(entity) {
                        snapshots.push(snapshot);
                    }
                }
                snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
                let _ = reply.send(match error {
                    Some(error) => Err(error),
                    None => Ok(snapshots),
                });
            }
            RegionOwnerLaneMessage::SimulationProjectionsForIds { entities, reply } => {
                let _ = reply.send(entity_projections::read(lane, &regions, entities));
            }
            RegionOwnerLaneMessage::DespawnProjectionsForIds { entities, reply } => {
                let _ = reply.send(entity_projections::read(lane, &regions, entities));
            }
            RegionOwnerLaneMessage::GoalCheckpointsForIds { entities, reply } => {
                let mut checkpoints = Vec::with_capacity(entities.len());
                let mut error = None;
                let mut ids_by_region = BTreeMap::<RegionKey, HashSet<EntityId>>::new();
                for (lease, entity) in entities {
                    if lease.lane != lane {
                        error = Some(RegionOwnerLaneError::WrongLane);
                        break;
                    }
                    let Some((current, _)) = regions.get(&lease.key) else {
                        error = Some(RegionOwnerLaneError::UnknownRegion);
                        break;
                    };
                    if *current != lease {
                        error = Some(RegionOwnerLaneError::StaleLease);
                        break;
                    }
                    ids_by_region.entry(lease.key).or_default().insert(entity);
                }
                if error.is_none() {
                    for (key, ids) in ids_by_region {
                        let Some((_, store)) = regions.get(&key) else {
                            error = Some(RegionOwnerLaneError::UnknownRegion);
                            break;
                        };
                        checkpoints.extend(store.goal_checkpoints_for_ids(&ids));
                    }
                }
                checkpoints.sort_unstable_by_key(|checkpoint| checkpoint.id);
                let _ = reply.send(match error {
                    Some(error) => Err(error),
                    None => Ok(checkpoints),
                });
            }
            RegionOwnerLaneMessage::AliveKinematicsForIds { entities, reply } => {
                let mut ids_by_region = BTreeMap::<RegionKey, HashSet<EntityId>>::new();
                let mut error = (pending.is_some() || committed.is_some())
                    .then_some(RegionOwnerLaneError::Busy);
                if error.is_none() {
                    for (lease, entity) in entities {
                        if lease.lane != lane {
                            error = Some(RegionOwnerLaneError::WrongLane);
                            break;
                        }
                        let Some((current, _)) = regions.get(&lease.key) else {
                            error = Some(RegionOwnerLaneError::UnknownRegion);
                            break;
                        };
                        if *current != lease {
                            error = Some(RegionOwnerLaneError::StaleLease);
                            break;
                        }
                        ids_by_region.entry(lease.key).or_default().insert(entity);
                    }
                }
                let mut candidates = Vec::new();
                let mut motion_states = Vec::new();
                if error.is_none() {
                    for (key, ids) in ids_by_region {
                        let mut ids = ids.into_iter().collect::<Vec<_>>();
                        ids.sort_unstable();
                        let (lease, store) = regions
                            .get(&key)
                            .expect("validated regional kinematics route");
                        let (mut region_candidates, mut region_motion_states) =
                            fenced_kinematics_candidates(store, *lease, &ids, true);
                        candidates.append(&mut region_candidates);
                        motion_states.append(&mut region_motion_states);
                    }
                    candidates.sort_unstable_by_key(|candidate| candidate.id);
                    motion_states.sort_unstable_by_key(|motion| motion.id);
                }
                let _ = reply.send(match error {
                    Some(error) => Err(error),
                    None => Ok(FencedKinematicsRead {
                        lane,
                        candidates,
                        motion_states,
                        state_version: state_version.load(Ordering::Acquire),
                    }),
                });
            }
            RegionOwnerLaneMessage::ApplyLocalKinematicsIfVersion { batches, reply } => {
                let result = if pending.is_some() || committed.is_some() {
                    Err(RegionOwnerLaneError::Busy)
                } else {
                    apply_local_kinematics_if_version(lane, &mut regions, batches).map(|states| {
                        states.map(|states| LocalKinematicsCommit {
                            states,
                            state_version: state_version.fetch_add(1, Ordering::AcqRel) + 1,
                        })
                    })
                };
                let _ = reply.send(result);
            }
            RegionOwnerLaneMessage::ApplyLocalPhysicsIfCurrent {
                expected_state_version,
                batches,
                reply,
            } => {
                let result = if pending.is_some() || committed.is_some() {
                    Err(RegionOwnerLaneError::Busy)
                } else {
                    let version_current =
                        state_version.load(Ordering::Acquire) == expected_state_version;
                    apply_local_physics_if_current(lane, &mut regions, batches, version_current)
                        .map(|(rejected, accepted, states)| {
                            let state_version = if states.is_empty() {
                                state_version.load(Ordering::Acquire)
                            } else {
                                state_version.fetch_add(1, Ordering::AcqRel) + 1
                            };
                            LocalPhysicsCommit {
                                rejected,
                                accepted,
                                states,
                                state_version,
                            }
                        })
                };
                let _ = reply.send(result);
            }
            RegionOwnerLaneMessage::TickOwnedRegions { input, reply } => {
                let result = if pending.is_some() || committed.is_some() {
                    Err(RegionOwnerLaneError::Busy)
                } else {
                    tick_owned_regions(lane, &mut regions, input).inspect(|output| {
                        if output.mutated {
                            state_version.fetch_add(1, Ordering::AcqRel);
                        }
                    })
                };
                let _ = reply.send(result);
            }
            RegionOwnerLaneMessage::TickOwnedRegionPhysics { input, reply } => {
                let result = if pending.is_some() || committed.is_some() {
                    Err(RegionOwnerLaneError::Busy)
                } else {
                    physics::tick_owned_region_physics(lane, &mut regions, input).map(
                        |mut output| {
                            output.state_version = if output.physics_mutated {
                                state_version.fetch_add(1, Ordering::AcqRel) + 1
                            } else {
                                state_version.load(Ordering::Acquire)
                            };
                            output
                        },
                    )
                };
                let _ = reply.send(result);
            }
            RegionOwnerLaneMessage::NearestVillager {
                leases,
                center,
                radius_squared,
                excluded,
                reply,
            } => {
                let mut error = (pending.is_some() || committed.is_some())
                    .then_some(RegionOwnerLaneError::Busy);
                if error.is_none() && (!center.is_finite() || !radius_squared.is_finite()) {
                    error = Some(RegionOwnerLaneError::InvalidQuery);
                }
                if error.is_none() && radius_squared < 0.0 {
                    error = Some(RegionOwnerLaneError::InvalidQuery);
                }
                if error.is_none() {
                    for lease in &leases {
                        if lease.lane != lane {
                            error = Some(RegionOwnerLaneError::WrongLane);
                            break;
                        }
                        let Some((current, _)) = regions.get(&lease.key) else {
                            error = Some(RegionOwnerLaneError::UnknownRegion);
                            break;
                        };
                        if current != lease {
                            error = Some(RegionOwnerLaneError::StaleLease);
                            break;
                        }
                    }
                }
                let mut nearest = None::<(f64, EntitySnapshot)>;
                if error.is_none() {
                    for lease in leases {
                        let store = &regions
                            .get(&lease.key)
                            .expect("validated nearest-villager region route")
                            .1;
                        for snapshot in store.snapshots() {
                            if snapshot.type_name != "minecraft:villager"
                                || snapshot.lifecycle != crate::EntityLifecycle::Alive
                                || excluded.contains(&snapshot.id)
                            {
                                continue;
                            }
                            let dx = snapshot.position.x - center.x;
                            let dy = snapshot.position.y - center.y;
                            let dz = snapshot.position.z - center.z;
                            let distance_squared = dx * dx + dy * dy + dz * dz;
                            if distance_squared > radius_squared {
                                continue;
                            }
                            let replace = nearest.as_ref().is_none_or(
                                |(nearest_distance, nearest_snapshot)| {
                                    distance_squared < *nearest_distance
                                        || (distance_squared == *nearest_distance
                                            && snapshot.id < nearest_snapshot.id)
                                },
                            );
                            if replace {
                                nearest = Some((distance_squared, snapshot));
                            }
                        }
                    }
                }
                let _ = reply.send(match error {
                    Some(error) => Err(error),
                    None => Ok(nearest.map(|(_, snapshot)| snapshot)),
                });
            }
            RegionOwnerLaneMessage::SaveBarrier {
                sequence_watermark,
                leases,
                reply,
            } => {
                let current_leases = regions
                    .iter()
                    .map(|(&key, (lease, _))| (key, *lease))
                    .collect::<BTreeMap<_, _>>();
                let expected_leases = leases
                    .into_iter()
                    .map(|lease| (lease.key, lease))
                    .collect::<BTreeMap<_, _>>();
                let result = if pending.is_some() || committed.is_some() {
                    Err(RegionOwnerLaneError::Busy)
                } else if last_sequence > sequence_watermark {
                    Err(RegionOwnerLaneError::StaleSequence)
                } else if current_leases != expected_leases {
                    Err(RegionOwnerLaneError::StaleLease)
                } else {
                    let mut snapshots = regions
                        .values()
                        .flat_map(|(_, store)| store.snapshots())
                        .collect::<Vec<_>>();
                    snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
                    Ok(snapshots)
                };
                let _ = reply.send(result);
            }
            RegionOwnerLaneMessage::PrepareGoalTick {
                lease,
                tick,
                active_ids,
                inputs,
                reply,
            } => {
                let result = if pending.is_some() || committed.is_some() {
                    Err(RegionOwnerLaneError::Busy)
                } else if lease.lane != lane {
                    Err(RegionOwnerLaneError::WrongLane)
                } else if let Some((current, store)) = regions.get_mut(&lease.key) {
                    if *current != lease {
                        Err(RegionOwnerLaneError::StaleLease)
                    } else {
                        let selection =
                            store.goal_tick_selection(lease.key, tick, &active_ids, &inputs);
                        let checkpoints = selection.checkpoints;
                        let goal_tick = selection.goal_tick;
                        let mut goal_overrides =
                            selection.goal_overrides.into_iter().collect::<Vec<_>>();
                        goal_overrides.sort_unstable_by_key(|(entity, _)| *entity);
                        Ok(PreparedRegionGoalTick {
                            checkpoints,
                            goal_tick,
                            goal_overrides,
                            pathing_aabbs: selection.pathing_aabbs,
                            snapshot_overrides: selection.snapshot_overrides,
                            villager_updates: selection.villager_updates,
                            cross_region_villager_candidates: selection
                                .cross_region_villager_candidates,
                        })
                    }
                } else {
                    Err(RegionOwnerLaneError::UnknownRegion)
                };
                let _ = reply.send(result);
            }
            #[cfg(test)]
            RegionOwnerLaneMessage::HoldForTest { entered, release } => {
                let _ = entered.send(());
                let _ = release.recv();
            }

            RegionOwnerLaneMessage::Shutdown { reply } => {
                if let Some(applied) = committed.take()
                    && let Err(error) = rollback_region_owner_batch(&mut regions, applied)
                {
                    let _ = reply.send(Err(error));
                    return;
                }
                let stores = regions
                    .into_iter()
                    .map(|(key, (_, store))| (key, store))
                    .collect();
                let _ = reply.send(Ok(stores));
                return;
            }
        }
    }
}

fn effect_expected_snapshot_matches(current: &EntitySnapshot, expected: &EntitySnapshot) -> bool {
    if current.health.is_nan() && expected.health.is_nan() {
        let mut current = current.clone();
        let mut expected = expected.clone();
        current.health = 0.0;
        expected.health = 0.0;
        current == expected
    } else {
        current == expected
    }
}

fn apply_local_kinematics_if_version(
    lane: usize,
    regions: &mut BTreeMap<RegionKey, (RegionLease, EntityStore)>,
    batches: Vec<LocalKinematicsRegionBatch>,
) -> Result<Option<Vec<EntityKinematics>>, RegionOwnerLaneError> {
    if batches.is_empty() {
        return Ok(Some(Vec::new()));
    }
    let mut all_ids = HashSet::new();
    for batch in &batches {
        if batch.lease.lane != lane {
            return Err(RegionOwnerLaneError::WrongLane);
        }
        let Some((current_lease, store)) = regions.get(&batch.lease.key) else {
            return Err(RegionOwnerLaneError::UnknownRegion);
        };
        if *current_lease != batch.lease {
            return Err(RegionOwnerLaneError::StaleLease);
        }
        if batch.previous.is_empty() || batch.previous.len() != batch.states.len() {
            return Ok(None);
        }
        let passengers = store.passenger_ids();
        let ids = batch
            .previous
            .iter()
            .map(|previous| previous.id)
            .collect::<HashSet<_>>();
        if ids.len() != batch.previous.len() || ids.iter().any(|id| !all_ids.insert(*id)) {
            return Ok(None);
        }
        let fences = store.kinematics_fence_states(&ids);
        if batch
            .previous
            .iter()
            .zip(&batch.states)
            .any(|(previous, state)| {
                let Some(current) = fences.get(&previous.id) else {
                    return true;
                };
                !previous.eligible()
                    || previous.id != state.id
                    || !state.is_finite()
                    || RegionKey::from_position(previous.expected.position) != Some(batch.lease.key)
                    || RegionKey::from_position(state.position) != Some(batch.lease.key)
                    || current.uuid != previous.uuid
                    || current.lifecycle != previous.lifecycle
                    || current.motion.position != previous.expected.position
                    || current.motion.rotation != previous.expected.rotation
                    || current.motion.velocity != previous.expected.velocity
                    || current.motion.on_ground != previous.expected.on_ground
                    || current.pickup_claimed
                    || current.vehicle_attached
                    || passengers.contains(&previous.id)
            })
        {
            return Ok(None);
        }
    }

    let mut committed = Vec::with_capacity(all_ids.len());
    for batch in batches {
        let expected_count = batch.states.len();
        let store = &mut regions
            .get_mut(&batch.lease.key)
            .expect("validated local kinematics region")
            .1;
        committed.extend(batch.states.iter().copied());
        if store.apply_kinematics_prevalidated(batch.states) != expected_count {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
    }
    committed.sort_unstable_by_key(|state| state.id);
    Ok(Some(committed))
}

fn tick_owned_regions(
    lane: usize,
    regions: &mut BTreeMap<RegionKey, (RegionLease, EntityStore)>,
    input: RegionalEntityTickInput,
) -> Result<OwnerLaneTickOutput, RegionOwnerLaneError> {
    let mut goal_stats = GoalTickStats::default();
    let mut active_entity_count = 0usize;
    let mut active_hostile_ids = Vec::new();
    let mut villager_population_candidates = Vec::new();
    let mut villager_ids = Vec::new();
    let mut villager_proximity_seeds = Vec::new();
    let mut fallback_entity_ids = HashSet::new();
    let mut goal_committed_motion = Vec::new();
    let mut physics_regions = Vec::new();
    let mut resolved_direct_paths = HashSet::new();
    let mut villager_profession_updates = Vec::new();
    let mut mutated = false;
    #[cfg(feature = "load-bench")]
    let profile_started = std::time::Instant::now();
    #[cfg(feature = "load-bench")]
    let elapsed_us = |started: std::time::Instant| {
        u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
    };
    #[cfg(feature = "load-bench")]
    let mut scan_us = 0_u64;
    #[cfg(feature = "load-bench")]
    let mut selection_us = 0_u64;
    #[cfg(feature = "load-bench")]
    let mut resolve_us = 0_u64;
    #[cfg(feature = "load-bench")]
    let mut resolve_scan_us = 0_u64;
    #[cfg(feature = "load-bench")]
    let mut resolve_pathing_us = 0_u64;
    #[cfg(feature = "load-bench")]
    let mut resolve_overrides_us = 0_u64;
    #[cfg(feature = "load-bench")]
    let mut resolve_map_us = 0_u64;
    #[cfg(feature = "load-bench")]
    let mut apply_us = 0_u64;
    #[cfg(feature = "load-bench")]
    let mut post_us = 0_u64;

    for (&key, (lease, store)) in regions.iter_mut() {
        #[cfg(feature = "load-bench")]
        let phase_started = std::time::Instant::now();
        if lease.lane != lane {
            return Err(RegionOwnerLaneError::WrongLane);
        }
        let store_capacity = store.len();
        let mut ordered_ids = Vec::with_capacity(store_capacity);
        let mut villager_job_sites = Vec::new();
        let mut selected_in_region = 0usize;
        store.visit_goal_tick_candidates(|state, lifecycle, local_living, villager, job_site| {
            let chunk = (
                (state.position.x.floor() as i32).div_euclid(16),
                (state.position.z.floor() as i32).div_euclid(16),
            );
            if lifecycle != crate::EntityLifecycle::Alive
                || !input.simulation_chunks.contains(&chunk)
            {
                return;
            }
            selected_in_region = selected_in_region.saturating_add(1);
            if local_living {
                if villager {
                    villager_job_sites.push((state.id, job_site));
                }
                ordered_ids.push(state.id);
            } else {
                fallback_entity_ids.insert(state.id);
            }
        });
        active_entity_count = active_entity_count.saturating_add(selected_in_region);
        if ordered_ids.is_empty() {
            continue;
        }

        let mut goals = input.goals.clone();
        if let Some(villager) = goals.villager.as_ref() {
            let missing_job_site_snapshot = villager_job_sites
                .iter()
                .filter_map(|(_, job_site)| *job_site)
                .any(|job_site| input.world.block_state_at(job_site).is_none());
            if missing_job_site_snapshot {
                fallback_entity_ids.extend(ordered_ids.iter().copied());
                continue;
            }
            let profession_offers = villager_job_sites
                .into_iter()
                .filter_map(|(id, job_site)| {
                    let state = input.world.block_state_at(job_site?)?;
                    input
                        .profession_offers_by_block_state
                        .get(&state)
                        .cloned()
                        .map(|offer| (id, offer))
                })
                .collect::<HashMap<_, _>>();
            goals.villager = Some(Arc::new(super::RegionalVillagerGoalTickInputs {
                day_time: villager.day_time,
                profile: Arc::clone(&villager.profile),
                profession_offers: Arc::new(profession_offers),
            }));
        }

        #[cfg(feature = "load-bench")]
        {
            scan_us = scan_us.saturating_add(elapsed_us(phase_started));
        }
        #[cfg(feature = "load-bench")]
        let phase_started = std::time::Instant::now();
        let selection =
            store.goal_tick_selection_for_ordered_ids(key, input.tick, &ordered_ids, &goals);
        #[cfg(feature = "load-bench")]
        {
            selection_us = selection_us.saturating_add(elapsed_us(phase_started));
        }
        #[cfg(feature = "load-bench")]
        let phase_started = std::time::Instant::now();
        #[cfg(feature = "load-bench")]
        let resolve_scan_started = std::time::Instant::now();
        let mut follow_targets = HashMap::new();
        let has_nonlocal_goal_reference = selection.checkpoints.iter().any(|checkpoint| {
            let Some(target) = goal_reference(&checkpoint.goal) else {
                return false;
            };
            let Some(target_view) = store.view(target) else {
                return true;
            };
            follow_targets.insert(target, target_view.position);
            false
        });
        if has_nonlocal_goal_reference
            || !selection.snapshot_overrides.is_empty()
            || !selection.cross_region_villager_candidates.is_empty()
        {
            fallback_entity_ids.extend(ordered_ids.iter().copied());
            continue;
        }

        #[cfg(feature = "load-bench")]
        {
            resolve_scan_us = resolve_scan_us.saturating_add(elapsed_us(resolve_scan_started));
        }
        #[cfg(feature = "load-bench")]
        let resolve_pathing_started = std::time::Instant::now();
        #[cfg(feature = "load-bench")]
        let resolve_map_started = std::time::Instant::now();
        let pathing_aabbs = selection
            .pathing_aabbs
            .into_iter()
            .collect::<HashMap<_, _>>();
        let pathing_probe = input.world.pathing_probe(&pathing_aabbs);
        #[cfg(feature = "load-bench")]
        {
            resolve_map_us = resolve_map_us.saturating_add(elapsed_us(resolve_map_started));
        }
        let resolved = selection
            .goal_tick
            .resolve_for_owner(&pathing_probe, input.pathing_budget);
        resolved_direct_paths.extend(pathing_probe.take_resolved_direct_paths());
        #[cfg(feature = "load-bench")]
        {
            resolve_pathing_us =
                resolve_pathing_us.saturating_add(elapsed_us(resolve_pathing_started));
        }
        #[cfg(feature = "load-bench")]
        let resolve_overrides_started = std::time::Instant::now();

        let mut goal_overrides = selection.goal_overrides.into_iter().collect::<Vec<_>>();
        goal_overrides.sort_unstable_by_key(|(entity, _)| *entity);
        let applied_goal_overrides = store.set_goals(goal_overrides.iter().cloned());
        assert_eq!(
            applied_goal_overrides,
            goal_overrides.len(),
            "owner-local goal overrides were selected from the same ECS store"
        );
        #[cfg(feature = "load-bench")]
        {
            resolve_us = resolve_us.saturating_add(elapsed_us(phase_started));
            resolve_overrides_us =
                resolve_overrides_us.saturating_add(elapsed_us(resolve_overrides_started));
        }
        #[cfg(feature = "load-bench")]
        let phase_started = std::time::Instant::now();
        let expected_capture_count = ordered_ids.len();
        let (stats, output) =
            store.apply_prepared_owner_goal_tick(resolved, follow_targets, ordered_ids);
        #[cfg(feature = "load-bench")]
        {
            apply_us = apply_us.saturating_add(elapsed_us(phase_started));
        }
        #[cfg(feature = "load-bench")]
        let phase_started = std::time::Instant::now();
        add_goal_tick_stats(&mut goal_stats, stats);
        if output.captured_count != expected_capture_count || output.invalid_count != 0 {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        active_hostile_ids.extend(output.active_hostile_ids);
        villager_population_candidates.extend(output.villager_population_candidates);
        villager_ids.extend(output.villager_ids);
        villager_proximity_seeds.extend(output.villager_proximity_seeds);
        goal_committed_motion.extend(
            output
                .goal_committed_motion
                .into_iter()
                .map(|motion| (*lease, motion)),
        );
        for update in selection.villager_updates {
            let profession_changed = update
                .expected
                .retained
                .villager
                .is_some_and(|previous| previous.profession != update.villager.profession);
            let mut current = store
                .snapshot(update.expected.id)
                .expect("owner-local villager update retains its selected entity");
            current.retained.villager = Some(update.villager);
            current.retained.villager_brain = Some(update.brain);
            current.retained.villager_gossip = update.gossip;
            current.retained.villager_merchant = update.merchant;
            let publication = profession_changed.then(|| current.clone());
            assert!(
                store.restore_snapshot_in_place(current),
                "owner-local villager update applies to the same ECS store"
            );
            villager_profession_updates.extend(publication);
        }

        physics_regions.push(*lease);
        mutated = true;
        #[cfg(feature = "load-bench")]
        {
            post_us = post_us.saturating_add(elapsed_us(phase_started));
        }
    }
    active_hostile_ids.sort_unstable_by_key(|(entity, _)| *entity);
    villager_population_candidates.sort_unstable_by_key(|(entity, _)| *entity);
    villager_ids.sort_unstable_by_key(|(entity, _)| *entity);
    villager_proximity_seeds.sort_unstable_by_key(|(entity, _)| *entity);
    goal_committed_motion.sort_unstable_by_key(|(_, motion)| motion.id);
    physics_regions.sort_unstable_by_key(|lease| lease.key);
    #[cfg(feature = "load-bench")]
    if input.tick.is_multiple_of(10) {
        eprintln!(
            "OWNER_GOAL_PHASE tick={} regions={} selected={} total_us={} scan_us={} selection_us={} resolve_us={} apply_us={} post_us={} resolve_scan_us={} resolve_pathing_us={} resolve_overrides_us={} resolve_map_us={}",
            input.tick,
            regions.len(),
            active_entity_count,
            elapsed_us(profile_started),
            scan_us,
            selection_us,
            resolve_us,
            apply_us,
            post_us,
            resolve_scan_us,
            resolve_pathing_us,
            resolve_overrides_us,
            resolve_map_us,
        );
    }
    Ok(OwnerLaneTickOutput {
        goal_stats,
        active_entity_count,
        active_hostile_ids,
        villager_population_candidates,
        villager_ids,
        villager_proximity_seeds,
        fallback_entity_ids,
        goal_committed_motion,
        physics_regions,
        resolved_direct_paths,
        villager_profession_updates,
        mutated,
    })
}

fn apply_local_physics_if_current(
    lane: usize,
    regions: &mut BTreeMap<RegionKey, (RegionLease, EntityStore)>,
    batches: Vec<LocalPhysicsRegionBatch>,
    version_current: bool,
) -> Result<LocalPhysicsApply, RegionOwnerLaneError> {
    for batch in &batches {
        if batch.lease.lane != lane {
            return Err(RegionOwnerLaneError::WrongLane);
        }
        let Some((current_lease, _)) = regions.get(&batch.lease.key) else {
            return Err(RegionOwnerLaneError::UnknownRegion);
        };
        if *current_lease != batch.lease {
            return Err(RegionOwnerLaneError::StaleLease);
        }
    }
    let input_count = batches.iter().map(|batch| batch.inputs.len()).sum();
    let mut rejected = Vec::new();
    let mut accepted = Vec::with_capacity(input_count);
    let mut committed = Vec::with_capacity(input_count);
    for batch in batches {
        if batch
            .world_fence
            .as_ref()
            .is_some_and(|world| !world.is_current())
        {
            rejected.extend(batch.inputs.iter().map(|input| input.previous.id));
            continue;
        }
        let store = &mut regions
            .get_mut(&batch.lease.key)
            .expect("validated local physics region")
            .1;
        // The coordinator's inverse vehicle index is authoritative, but keep
        // the owner-local passenger relation as the final mutation fence.
        let mut passengers = None;
        let mut states = Vec::with_capacity(batch.inputs.len());
        for LocalPhysicsInput {
            previous,
            expected,
            step,
            publish,
        } in batch.inputs
        {
            if previous.lifecycle != crate::EntityLifecycle::Alive
                || previous.pickup_claimed
                || previous.vehicle_attached
                || previous.id != expected.id
                || expected.id != step.id
                || !step.position.is_finite()
                || !step.velocity.is_finite()
                || RegionKey::from_position(expected.position) != Some(batch.lease.key)
                || RegionKey::from_position(step.position) != Some(batch.lease.key)
            {
                rejected.push(previous.id);
                continue;
            }

            let unchanged = expected.position == step.position
                && expected.velocity == step.velocity
                && expected.on_ground == step.on_ground;
            let ordinary_living = matches!(
                expected.kind,
                crate::EntityPhysicsKind::Living
                    | crate::EntityPhysicsKind::PowderSnowWalkableLiving
                    | crate::EntityPhysicsKind::FishLiving
                    | crate::EntityPhysicsKind::SquidLiving
                    | crate::EntityPhysicsKind::AquaticLiving
            );
            if version_current && ordinary_living && !step.horizontal_collision {
                let motion = EntityMotionState {
                    id: expected.id,
                    position: expected.position,
                    rotation: previous.rotation,
                    velocity: expected.velocity,
                    on_ground: expected.on_ground,
                    fall_distance: expected.fall_distance,
                    goal_fence: expected.goal_fence,
                    is_item: false,
                    is_experience: false,
                    is_arrow: false,
                    arrow_revision: None,
                    arrow_embedded_block: None,
                    is_hurting_projectile: false,
                    hurting_projectile_revision: None,
                    is_throwable_projectile: false,
                    throwable_projectile_revision: None,
                    sends_velocity: true,
                };
                if publish {
                    accepted.push((batch.lease, motion));
                }
                if !unchanged {
                    let state = EntityKinematics {
                        id: step.id,
                        position: step.position,
                        rotation: motion.rotation,
                        velocity: step.velocity,
                        on_ground: step.on_ground,
                    };
                    if publish {
                        committed.push(state);
                    }
                    states.push(state);
                }
                continue;
            }

            let passengers = passengers.get_or_insert_with(|| store.passenger_ids());
            let Some(current) = store.kinematics_fence_state(previous.id) else {
                rejected.push(previous.id);
                continue;
            };
            let motion = current.motion;
            if current.uuid != previous.uuid
                || current.lifecycle != previous.lifecycle
                || current.pickup_claimed
                || passengers.contains(&previous.id)
                || current.vehicle_attached
                || !expected.matches_motion(motion)
            {
                rejected.push(previous.id);
                continue;
            }
            let changed = motion.position != step.position
                || motion.velocity != step.velocity
                || motion.on_ground != step.on_ground;
            let projectile =
                motion.is_arrow || motion.is_hurting_projectile || motion.is_throwable_projectile;
            let should_publish = publish
                || projectile
                || motion.is_item
                || motion.is_experience
                || step.horizontal_collision
                || matches!(expected.kind, crate::EntityPhysicsKind::FallingBlock);
            if should_publish {
                accepted.push((batch.lease, motion));
            }
            if changed && !projectile {
                let state = EntityKinematics {
                    id: step.id,
                    position: step.position,
                    rotation: motion.rotation,
                    velocity: step.velocity,
                    on_ground: step.on_ground,
                };
                if should_publish {
                    committed.push(state);
                }
                states.push(state);
            }
        }
        let expected_count = states.len();
        if store.apply_kinematics_prevalidated(states) != expected_count {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
    }
    rejected.sort_unstable();
    accepted.sort_unstable_by_key(|(_, motion)| motion.id);
    committed.sort_unstable_by_key(|state| state.id);
    Ok((rejected, accepted, committed))
}

fn prepare_region_owner_batch(
    lane: usize,
    regions: &BTreeMap<RegionKey, (RegionLease, EntityStore)>,
    last_phase: u64,
    last_sequence: u64,
    current_state_version: u64,
    mut batch: RegionOwnerBatch,
) -> Result<RegionOwnerBatch, RegionOwnerLaneError> {
    if batch.phase.0 <= last_phase {
        return Err(RegionOwnerLaneError::StalePhase);
    }
    batch
        .mutations
        .sort_by_key(|mutation| (mutation.lease.key, mutation.sequence));
    if batch.sequence_watermark < last_sequence {
        return Err(RegionOwnerLaneError::DuplicateSequence);
    }
    let mut sequences = HashSet::with_capacity(batch.mutations.len());
    let mut inserted_ids = HashSet::new();
    let mut inserted_uuids = HashSet::new();
    let mut passengers_by_region = HashMap::<RegionKey, HashSet<EntityId>>::new();
    for mutation in &batch.mutations {
        if !sequences.insert(mutation.sequence) {
            return Err(RegionOwnerLaneError::DuplicateSequence);
        }
        if mutation.sequence <= last_sequence {
            return Err(RegionOwnerLaneError::DuplicateSequence);
        }
        if mutation.sequence > batch.sequence_watermark {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        if mutation.lease.lane != lane {
            return Err(RegionOwnerLaneError::WrongLane);
        }
        let Some((current, store)) = regions.get(&mutation.lease.key) else {
            return Err(RegionOwnerLaneError::UnknownRegion);
        };
        if *current != mutation.lease {
            return Err(RegionOwnerLaneError::StaleLease);
        }
        match &mutation.mutation {
            RegionOwnerMutation::SetVelocity { entity, velocity } => {
                if !velocity.is_finite() {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                if !store.contains(*entity) {
                    return Err(RegionOwnerLaneError::UnknownEntity);
                }
            }
            RegionOwnerMutation::SetAnimalState { entity, .. } => {
                if store
                    .snapshot(*entity)
                    .and_then(|snapshot| snapshot.animal)
                    .is_none()
                {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
            }
            RegionOwnerMutation::SetAnimalStateIfCurrent { expected, .. } => {
                if expected.animal.is_none()
                    || store.snapshot(expected.id).as_ref() != Some(expected.as_ref())
                {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
            }
            RegionOwnerMutation::SetGrazingStateIfCurrent {
                expected, velocity, ..
            } => {
                if velocity.is_some_and(|velocity| !velocity.is_finite())
                    || store.snapshot(expected.id).as_ref() != Some(expected.as_ref())
                {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
            }
            RegionOwnerMutation::SetGoalIfCurrent { expected, .. }
            | RegionOwnerMutation::SetItemStackIfCurrent { expected, .. } => {
                if store.snapshot(expected.id).as_ref() != Some(expected.as_ref()) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
            }
            RegionOwnerMutation::ReplaceSnapshotIfCurrent {
                expected,
                next,
                allow_type_change,
            } => {
                let type_change_valid = if *allow_type_change {
                    expected.type_id != next.type_id
                        && expected.type_name != next.type_name
                        && expected.position == next.position
                } else {
                    expected.type_id == next.type_id && expected.type_name == next.type_name
                };
                if expected.id != next.id
                    || expected.uuid != next.uuid
                    || !type_change_valid
                    || RegionKey::from_position(next.position) != Some(mutation.lease.key)
                    || store.snapshot(expected.id).as_ref() != Some(expected.as_ref())
                {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
            }
            RegionOwnerMutation::SetKinematicsIfCurrent { expected, state } => {
                if expected.id != state.id
                    || !state.is_finite()
                    || RegionKey::from_position(state.position) != Some(mutation.lease.key)
                    || store.snapshot(expected.id).as_ref() != Some(expected.as_ref())
                {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
            }
            RegionOwnerMutation::SetKinematicsBatchIfCurrent { expected, states } => {
                let mut ids = HashSet::with_capacity(expected.len());
                if expected.is_empty()
                    || expected.len() != states.len()
                    || expected.iter().zip(states).any(|(expected, state)| {
                        expected.id != state.id
                            || !ids.insert(expected.id)
                            || !state.is_finite()
                            || RegionKey::from_position(state.position) != Some(mutation.lease.key)
                            || store.snapshot(expected.id).as_ref() != Some(expected)
                    })
                {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
            }
            RegionOwnerMutation::SetKinematicsBatchIfVersion {
                expected_state_version,
                previous,
                states,
            } => {
                let passengers = passengers_by_region
                    .entry(mutation.lease.key)
                    .or_insert_with(|| store.passenger_ids());
                #[cfg(feature = "load-bench")]
                let lane_version_stale = *expected_state_version != current_state_version;
                #[cfg(not(feature = "load-bench"))]
                let _ = expected_state_version;
                // A lane version also advances for unrelated entities (for
                // example, player movement sharing this lane). The compact
                // target fence below is the authoritative stale check: it
                // still rejects changed identity, motion, ownership, pickup,
                // and passenger state without turning disjoint lane traffic
                // into a full-snapshot fallback.
                let invalid = if previous.is_empty() || previous.len() != states.len() {
                    true
                } else {
                    let mut ids = HashSet::with_capacity(previous.len());
                    let duplicate = previous.iter().any(|previous| !ids.insert(previous.id));
                    let fences = store.kinematics_fence_states(&ids);
                    duplicate
                        || previous.iter().zip(states).any(|(previous, state)| {
                            let Some(current) = fences.get(&previous.id) else {
                                return true;
                            };
                            !previous.eligible()
                                || previous.id != state.id
                                || !state.is_finite()
                                || RegionKey::from_position(previous.expected.position)
                                    != Some(mutation.lease.key)
                                || RegionKey::from_position(state.position)
                                    != Some(mutation.lease.key)
                                || current.uuid != previous.uuid
                                || current.lifecycle != previous.lifecycle
                                || current.motion.position != previous.expected.position
                                || current.motion.rotation != previous.expected.rotation
                                || current.motion.velocity != previous.expected.velocity
                                || current.motion.on_ground != previous.expected.on_ground
                                || current.pickup_claimed
                                || current.vehicle_attached
                                || passengers.contains(&previous.id)
                        })
                };
                #[cfg(feature = "load-bench")]
                if invalid {
                    eprintln!(
                        "PHYSICS_VERSIONED_LANE_REJECT lane={} region={:?} lane_stale={} expected_version={} current_version={} previous={} states={}",
                        lane,
                        mutation.lease.key,
                        lane_version_stale,
                        expected_state_version,
                        current_state_version,
                        previous.len(),
                        states.len()
                    );
                }
                if invalid {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
            }
            RegionOwnerMutation::DamageIfCurrent { expected, request } => {
                let passengers = passengers_by_region
                    .entry(mutation.lease.key)
                    .or_insert_with(|| {
                        store
                            .views()
                            .filter_map(|view| view.vehicle.and_then(|vehicle| vehicle.passenger))
                            .collect()
                    });
                if !request.is_valid()
                    || store.snapshot(expected.id).as_ref() != Some(expected.as_ref())
                    || expected.lifecycle != crate::EntityLifecycle::Alive
                    || passengers.contains(&expected.id)
                {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
            }
            RegionOwnerMutation::ApplyEffectIfCurrent { expected, .. } => {
                let Some(current) = store.snapshot(expected.id) else {
                    return Err(RegionOwnerLaneError::UnknownEntity);
                };
                if !effect_expected_snapshot_matches(&current, expected) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
            }
            RegionOwnerMutation::ApplyGoalBatch {
                expected,
                candidate_ids,
                expected_state_version,
                resolved,
                goal_overrides,
                snapshot_overrides,
                villager_updates,
                ..
            } => {
                let expected_ids_are_unique =
                    expected.windows(2).all(|pair| pair[0].id < pair[1].id);
                let candidate_ids_are_unique =
                    candidate_ids.windows(2).all(|pair| pair[0] < pair[1]);
                let contains_expected = |id: &EntityId| {
                    expected
                        .binary_search_by_key(id, |checkpoint| checkpoint.id)
                        .is_ok()
                };
                let override_ids = goal_overrides
                    .iter()
                    .map(|(entity, _)| *entity)
                    .collect::<HashSet<_>>();
                let snapshot_override_ids = snapshot_overrides
                    .iter()
                    .map(|(expected, _)| expected.id)
                    .collect::<HashSet<_>>();
                let villager_update_ids = villager_updates
                    .iter()
                    .map(|update| update.expected.id)
                    .collect::<HashSet<_>>();
                let resolved_ids_match = resolved.active_ids.as_ref().is_some_and(|active| {
                    active.union(&override_ids).count() == expected.len()
                        && expected.iter().all(|checkpoint| {
                            active.contains(&checkpoint.id) || override_ids.contains(&checkpoint.id)
                        })
                });
                // Goal preparation captures this lane version with the rollback
                // checkpoints, and the coordinator rechecks it immediately before
                // this single-threaded lane admits the batch. Every successful
                // owner mutation advances the version before replying, so equality
                // here is the exact stale-state fence; the full checkpoints remain
                // only for rollback.
                if *expected_state_version != current_state_version
                    || !expected_ids_are_unique
                    || !candidate_ids_are_unique
                    || candidate_ids.iter().any(|id| !store.contains(*id))
                    || override_ids.len() != goal_overrides.len()
                    || snapshot_override_ids.len() != snapshot_overrides.len()
                    || villager_update_ids.len() != villager_updates.len()
                    || !override_ids.iter().all(&contains_expected)
                    || !snapshot_override_ids.iter().all(&contains_expected)
                    || !villager_update_ids.iter().all(&contains_expected)
                    || expected.iter().any(|checkpoint| {
                        RegionKey::from_position(checkpoint.position) != Some(mutation.lease.key)
                    })
                    || snapshot_overrides.iter().any(|(expected, next)| {
                        let mut allowed = expected.clone();
                        allowed.velocity = next.velocity;
                        allowed.rotation = next.rotation;
                        allowed.retained.hurting_projectile_state =
                            next.retained.hurting_projectile_state;
                        expected.id != next.id
                            || expected.uuid != next.uuid
                            || expected.type_name != "minecraft:shulker_bullet"
                            || expected.retained.shulker_bullet.is_none()
                            || !next.rotation.is_finite()
                            || !next.velocity.is_finite()
                            || RegionKey::from_position(next.position) != Some(mutation.lease.key)
                            || store.snapshot(expected.id).as_ref() != Some(expected)
                            || &allowed != next
                    })
                    || villager_updates.iter().any(|update| {
                        let pois = update.brain.pois;
                        update.expected.type_name != "minecraft:villager"
                            || update.expected.retained.villager.is_none()
                            || RegionKey::from_position(update.expected.position)
                                != Some(mutation.lease.key)
                            || store.snapshot(update.expected.id).as_ref() != Some(&update.expected)
                            || [pois.home, pois.job_site, pois.meeting_point]
                                .into_iter()
                                .flatten()
                                .any(|position| !position.is_finite())
                    })
                    || (resolved.active_ids.is_some() && !resolved_ids_match)
                    || (resolved.active_ids.is_none() && expected.len() != store.len())
                {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
            }
            RegionOwnerMutation::InsertSnapshot(snapshot) => {
                if RegionKey::from_position(snapshot.position) != Some(mutation.lease.key)
                    || !snapshot.rotation.is_finite()
                    || !snapshot.velocity.is_finite()
                    || store.contains(snapshot.id)
                    || store.contains_uuid(snapshot.uuid)
                    || !inserted_ids.insert(snapshot.id)
                    || !inserted_uuids.insert(snapshot.uuid)
                    || snapshot_vehicle_reference(snapshot).is_some()
                    || goal_reference(&snapshot.goal).is_some()
                {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
            }
            RegionOwnerMutation::InsertSnapshots(snapshots) => {
                if snapshots.is_empty()
                    || snapshots.iter().any(|snapshot| {
                        RegionKey::from_position(snapshot.position) != Some(mutation.lease.key)
                            || !snapshot.rotation.is_finite()
                            || !snapshot.velocity.is_finite()
                            || store.contains(snapshot.id)
                            || store.contains_uuid(snapshot.uuid)
                            || !inserted_ids.insert(snapshot.id)
                            || !inserted_uuids.insert(snapshot.uuid)
                    })
                {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                let available = store
                    .snapshots()
                    .map(|snapshot| snapshot.id)
                    .chain(snapshots.iter().map(|snapshot| snapshot.id))
                    .collect::<HashSet<_>>();
                if snapshots.iter().any(|snapshot| {
                    snapshot_vehicle_reference(snapshot)
                        .is_some_and(|passenger| !available.contains(&passenger))
                }) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
            }
            RegionOwnerMutation::RemoveEntity(entity) => {
                if !store.contains(*entity) {
                    return Err(RegionOwnerLaneError::UnknownEntity);
                }
                if store
                    .snapshots()
                    .any(|snapshot| snapshot_vehicle_reference(&snapshot) == Some(*entity))
                {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
            }
            RegionOwnerMutation::RemoveIfCurrent(expected) => {
                if store.snapshot(expected.id).as_ref() != Some(expected.as_ref())
                    || store
                        .snapshots()
                        .any(|snapshot| snapshot_vehicle_reference(&snapshot) == Some(expected.id))
                {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
            }
            RegionOwnerMutation::RemoveSnapshotsIfCurrent(expected) => {
                let ids = expected
                    .iter()
                    .map(|snapshot| snapshot.id)
                    .collect::<HashSet<_>>();
                if expected.is_empty()
                    || ids.len() != expected.len()
                    || expected.iter().any(|snapshot| {
                        RegionKey::from_position(snapshot.position) != Some(mutation.lease.key)
                            || store.snapshot(snapshot.id).as_ref() != Some(snapshot)
                    })
                    || store.snapshots().any(|snapshot| {
                        !ids.contains(&snapshot.id)
                            && snapshot_vehicle_reference(&snapshot)
                                .is_some_and(|passenger| ids.contains(&passenger))
                    })
                {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
            }
        }
    }
    Ok(batch)
}

fn apply_prepared_region_owner_batch(
    regions: &mut BTreeMap<RegionKey, (RegionLease, EntityStore)>,
    batch: RegionOwnerBatch,
) -> Result<(RegionOwnerCompletion, CommittedRegionOwnerBatch), RegionOwnerLaneError> {
    let mut applied_sequences = Vec::with_capacity(batch.mutations.len());
    let mut undo = Vec::with_capacity(batch.mutations.len());
    let mut goal_stats = GoalTickStats::default();
    let mut effect_results = Vec::new();
    let mut goal_candidate_ids = BTreeMap::<RegionKey, Vec<EntityId>>::new();
    let mut captured_goal_candidates =
        BTreeMap::<RegionKey, Vec<crate::runtime::GoalSimulationCandidate>>::new();
    for mutation in batch.mutations {
        let store = &mut regions
            .get_mut(&mutation.lease.key)
            .expect("owner batch regions were preflighted")
            .1;
        let applied = match mutation.mutation {
            RegionOwnerMutation::SetVelocity { entity, velocity } => {
                let snapshot = store
                    .snapshot(entity)
                    .ok_or(RegionOwnerLaneError::UnknownEntity)?;
                undo.push(RegionOwnerUndo::Velocity {
                    lease: mutation.lease,
                    entity,
                    velocity: snapshot.velocity,
                });
                store.set_velocity(entity, velocity)
            }
            RegionOwnerMutation::SetAnimalState { entity, animal } => {
                let snapshot = store
                    .snapshot(entity)
                    .ok_or(RegionOwnerLaneError::UnknownEntity)?;
                undo.push(RegionOwnerUndo::AnimalState {
                    lease: mutation.lease,
                    entity,
                    animal: snapshot
                        .animal
                        .ok_or(RegionOwnerLaneError::InvalidMutation)?,
                });
                store.set_animal_state(entity, animal)
            }
            RegionOwnerMutation::SetAnimalStateIfCurrent { expected, animal } => {
                let entity = expected.id;
                undo.push(RegionOwnerUndo::AnimalState {
                    lease: mutation.lease,
                    entity,
                    animal: expected
                        .animal
                        .ok_or(RegionOwnerLaneError::InvalidMutation)?,
                });
                store.set_animal_state(entity, animal)
            }
            RegionOwnerMutation::SetGrazingStateIfCurrent {
                expected,
                velocity,
                remaining_ticks,
            } => {
                let mut next = expected.as_ref().clone();
                if let Some(velocity) = velocity {
                    next.velocity = velocity;
                }
                next.retained.sheep_grazing_ticks = remaining_ticks;
                let applied = store.restore_snapshot_in_place(next);
                if applied {
                    undo.push(RegionOwnerUndo::Snapshot {
                        lease: mutation.lease,
                        snapshot: expected,
                        allow_type_change: false,
                    });
                }
                applied
            }
            RegionOwnerMutation::SetGoalIfCurrent { expected, goal } => {
                let entity = expected.id;
                undo.push(RegionOwnerUndo::Goal {
                    lease: mutation.lease,
                    snapshot: expected,
                });
                store.set_goal(entity, goal)
            }
            RegionOwnerMutation::SetItemStackIfCurrent {
                expected,
                item_stack,
            } => {
                let entity = expected.id;
                undo.push(RegionOwnerUndo::ItemStack {
                    lease: mutation.lease,
                    entity,
                    item_stack: expected.item_stack,
                });
                store.set_item_stack(entity, item_stack)
            }
            RegionOwnerMutation::ReplaceSnapshotIfCurrent {
                expected,
                next,
                allow_type_change,
            } => {
                undo.push(RegionOwnerUndo::Snapshot {
                    lease: mutation.lease,
                    snapshot: expected,
                    allow_type_change,
                });
                if allow_type_change {
                    store.convert_snapshot_in_place(*next)
                } else {
                    store.restore_snapshot_in_place(*next)
                }
            }
            RegionOwnerMutation::SetKinematicsIfCurrent { expected, state } => {
                undo.push(RegionOwnerUndo::Kinematics {
                    lease: mutation.lease,
                    state: EntityKinematics {
                        id: expected.id,
                        position: expected.position,
                        rotation: expected.rotation,
                        velocity: expected.velocity,
                        on_ground: expected.on_ground,
                    },
                });
                store.apply_kinematics([state]) == 1
            }
            RegionOwnerMutation::SetKinematicsBatchIfCurrent { expected, states } => {
                let previous = expected
                    .iter()
                    .map(|snapshot| EntityKinematics {
                        id: snapshot.id,
                        position: snapshot.position,
                        rotation: snapshot.rotation,
                        velocity: snapshot.velocity,
                        on_ground: snapshot.on_ground,
                    })
                    .collect::<Vec<_>>();
                let expected_count = states.len();
                let applied = store.apply_kinematics_prevalidated(states) == expected_count;
                if applied {
                    undo.push(RegionOwnerUndo::KinematicsBatch {
                        lease: mutation.lease,
                        states: previous,
                    });
                }
                applied
            }
            RegionOwnerMutation::SetKinematicsBatchIfVersion {
                expected_state_version: _,
                previous,
                states,
            } => {
                let expected_count = states.len();
                let applied = store.apply_kinematics_prevalidated(states) == expected_count;
                if applied {
                    undo.push(RegionOwnerUndo::KinematicsBatch {
                        lease: mutation.lease,
                        states: previous
                            .into_iter()
                            .map(|previous| previous.expected)
                            .collect(),
                    });
                }
                applied
            }
            RegionOwnerMutation::DamageIfCurrent { expected, request } => {
                let entity = expected.id;
                let applied = store.damage(entity, request).is_some();
                if applied {
                    undo.push(RegionOwnerUndo::Damaged {
                        lease: mutation.lease,
                        expected,
                    });
                }
                applied
            }
            RegionOwnerMutation::ApplyEffectIfCurrent { expected, request } => {
                let checkpoint = store
                    .effect_checkpoint(expected.id)
                    .ok_or(RegionOwnerLaneError::UnknownEntity)?;
                let result = store.apply_effect(expected.id, *request);
                if matches!(result, EntityEffectResult::Applied(_)) {
                    undo.push(RegionOwnerUndo::Effect {
                        lease: mutation.lease,
                        checkpoint: Box::new(checkpoint),
                    });
                }
                effect_results.push((mutation.sequence, result));
                true
            }
            RegionOwnerMutation::ApplyGoalBatch {
                expected,
                candidate_ids,
                expected_state_version: _,
                resolved,
                follow_targets,
                goal_overrides,
                snapshot_overrides,
                villager_updates,
            } => {
                if goal_candidate_ids.contains_key(&mutation.lease.key) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                let checkpoints = expected;
                if store.set_goals(goal_overrides.iter().cloned()) != goal_overrides.len() {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                let capture_is_current = snapshot_overrides.is_empty();
                let (stats, captured) = store
                    .apply_prepared_goal_tick_with_follow_targets_and_simulation_results(
                        *resolved,
                        &follow_targets,
                        candidate_ids.clone(),
                    );
                add_goal_tick_stats(&mut goal_stats, stats);
                goal_candidate_ids.insert(mutation.lease.key, candidate_ids);
                if capture_is_current {
                    captured_goal_candidates.insert(mutation.lease.key, captured);
                }
                undo.push(RegionOwnerUndo::GoalBatch {
                    lease: mutation.lease,
                    checkpoints,
                });
                let mut applied = true;
                for (expected, retargeted) in snapshot_overrides {
                    let Some(current) = store.snapshot(expected.id) else {
                        applied = false;
                        break;
                    };
                    let mut next = current.clone();
                    next.velocity = retargeted.velocity;
                    next.rotation = retargeted.rotation;
                    next.retained.hurting_projectile_state =
                        retargeted.retained.hurting_projectile_state;
                    if !store.restore_snapshot_in_place(next) {
                        applied = false;
                        break;
                    }
                    undo.push(RegionOwnerUndo::Snapshot {
                        lease: mutation.lease,
                        snapshot: Box::new(current),
                        allow_type_change: false,
                    });
                }
                if applied {
                    for update in villager_updates {
                        let Some(current) = store.snapshot(update.expected.id) else {
                            applied = false;
                            break;
                        };
                        let mut next = current.clone();
                        next.retained.villager = Some(update.villager);
                        next.retained.villager_brain = Some(update.brain);
                        next.retained.villager_gossip = update.gossip;
                        next.retained.villager_merchant = update.merchant;
                        if !store.restore_snapshot_in_place(next) {
                            applied = false;
                            break;
                        }
                        undo.push(RegionOwnerUndo::Snapshot {
                            lease: mutation.lease,
                            snapshot: Box::new(current),
                            allow_type_change: false,
                        });
                    }
                }
                applied
            }
            RegionOwnerMutation::InsertSnapshot(snapshot) => {
                let entity = snapshot.id;
                let previous_next_id = store.next_id;
                let inserted = store.insert_snapshot(*snapshot);
                if inserted {
                    undo.push(RegionOwnerUndo::Inserted {
                        lease: mutation.lease,
                        entity,
                        previous_next_id,
                    });
                }
                inserted
            }
            RegionOwnerMutation::InsertSnapshots(snapshots) => {
                let entities = snapshots
                    .iter()
                    .map(|snapshot| snapshot.id)
                    .collect::<Vec<_>>();
                let previous_next_id = store.next_id;
                let inserted = store.insert_snapshots_batch(snapshots);
                if inserted {
                    undo.push(RegionOwnerUndo::InsertedBatch {
                        lease: mutation.lease,
                        entities,
                        previous_next_id,
                    });
                }
                inserted
            }
            RegionOwnerMutation::RemoveEntity(entity) => {
                let Some(snapshot) = store.remove(entity) else {
                    return Err(RegionOwnerLaneError::UnknownEntity);
                };
                undo.push(RegionOwnerUndo::Removed {
                    lease: mutation.lease,
                    snapshot: Box::new(snapshot),
                });
                true
            }
            RegionOwnerMutation::RemoveIfCurrent(expected) => {
                let Some(snapshot) = store.remove(expected.id) else {
                    return Err(RegionOwnerLaneError::UnknownEntity);
                };
                undo.push(RegionOwnerUndo::Removed {
                    lease: mutation.lease,
                    snapshot: Box::new(snapshot),
                });
                true
            }
            RegionOwnerMutation::RemoveSnapshotsIfCurrent(expected) => {
                let ordered = order_vehicle_group_for_removal(&expected);
                let mut removed = Vec::with_capacity(ordered.len());
                let mut success = true;
                for entity in ordered {
                    let Some(snapshot) = store.remove(entity) else {
                        success = false;
                        break;
                    };
                    removed.push(snapshot);
                }
                if success {
                    undo.push(RegionOwnerUndo::RemovedBatch {
                        lease: mutation.lease,
                        snapshots: expected,
                    });
                } else if !removed.is_empty() && !store.insert_snapshots_batch(removed) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                success
            }
        };
        if !applied {
            let rollback = rollback_region_owner_undo(regions, undo);
            rollback?;
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        applied_sequences.push(mutation.sequence);
    }
    let mut goal_candidate_runs = Vec::with_capacity(goal_candidate_ids.len());
    for (key, ids) in goal_candidate_ids {
        let (lease, store) = regions
            .get(&key)
            .expect("applied goal batch retains its regional store");
        let candidates = captured_goal_candidates
            .remove(&key)
            .and_then(|captured| fenced_kinematics_candidates_from_capture(*lease, &ids, captured))
            .unwrap_or_else(|| fenced_kinematics_candidates(store, *lease, &ids, false).0);
        goal_candidate_runs.push(candidates);
    }
    let completion = RegionOwnerCompletion {
        phase: batch.phase,
        goal_candidate_runs,
        applied_sequences,
        goal_stats,
        effect_results,
    };
    let applied = CommittedRegionOwnerBatch {
        phase: batch.phase,
        sequence_watermark: batch.sequence_watermark,
        undo,
    };
    Ok((completion, applied))
}

fn rollback_region_owner_batch(
    regions: &mut BTreeMap<RegionKey, (RegionLease, EntityStore)>,
    applied: CommittedRegionOwnerBatch,
) -> Result<(), RegionOwnerLaneError> {
    rollback_region_owner_undo(regions, applied.undo)
}

fn rollback_region_owner_undo(
    regions: &mut BTreeMap<RegionKey, (RegionLease, EntityStore)>,
    undo: Vec<RegionOwnerUndo>,
) -> Result<(), RegionOwnerLaneError> {
    for change in undo.into_iter().rev() {
        let (lease, entity, restored) = match change {
            RegionOwnerUndo::Velocity {
                lease,
                entity,
                velocity,
            } => (
                lease,
                entity,
                RegionOwnerMutation::SetVelocity { entity, velocity },
            ),
            RegionOwnerUndo::AnimalState {
                lease,
                entity,
                animal,
            } => (
                lease,
                entity,
                RegionOwnerMutation::SetAnimalState { entity, animal },
            ),
            RegionOwnerUndo::Inserted {
                lease,
                entity,
                previous_next_id,
            } => {
                let store = &mut regions
                    .get_mut(&lease.key)
                    .ok_or(RegionOwnerLaneError::UnknownRegion)?
                    .1;
                if store.remove(entity).is_none() {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                store.next_id = previous_next_id;
                continue;
            }
            RegionOwnerUndo::InsertedBatch {
                lease,
                entities,
                previous_next_id,
            } => {
                let store = &mut regions
                    .get_mut(&lease.key)
                    .ok_or(RegionOwnerLaneError::UnknownRegion)?
                    .1;
                for entity in entities {
                    if store.remove(entity).is_none() {
                        return Err(RegionOwnerLaneError::InvalidMutation);
                    }
                }
                store.next_id = previous_next_id;
                continue;
            }
            RegionOwnerUndo::Removed { lease, snapshot } => {
                let store = &mut regions
                    .get_mut(&lease.key)
                    .ok_or(RegionOwnerLaneError::UnknownRegion)?
                    .1;
                if !store.insert_snapshot(*snapshot) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                continue;
            }
            RegionOwnerUndo::Goal { lease, snapshot } => {
                let store = &mut regions
                    .get_mut(&lease.key)
                    .ok_or(RegionOwnerLaneError::UnknownRegion)?
                    .1;
                if !store.restore_snapshot_in_place(*snapshot) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                continue;
            }
            RegionOwnerUndo::ItemStack {
                lease,
                entity,
                item_stack,
            } => {
                let store = &mut regions
                    .get_mut(&lease.key)
                    .ok_or(RegionOwnerLaneError::UnknownRegion)?
                    .1;
                if !store.set_item_stack(entity, item_stack) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                continue;
            }
            RegionOwnerUndo::RemovedBatch { lease, snapshots } => {
                let store = &mut regions
                    .get_mut(&lease.key)
                    .ok_or(RegionOwnerLaneError::UnknownRegion)?
                    .1;
                if !store.insert_snapshots_batch(snapshots) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                continue;
            }
            RegionOwnerUndo::Kinematics { lease, state } => {
                let store = &mut regions
                    .get_mut(&lease.key)
                    .ok_or(RegionOwnerLaneError::UnknownRegion)?
                    .1;
                if store.apply_kinematics([state]) != 1 {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                continue;
            }
            RegionOwnerUndo::KinematicsBatch { lease, states } => {
                let store = &mut regions
                    .get_mut(&lease.key)
                    .ok_or(RegionOwnerLaneError::UnknownRegion)?
                    .1;
                let expected = states.len();
                if store.apply_kinematics(states) != expected {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                continue;
            }
            RegionOwnerUndo::Damaged { lease, expected } => {
                let store = &mut regions
                    .get_mut(&lease.key)
                    .ok_or(RegionOwnerLaneError::UnknownRegion)?
                    .1;
                if !store.restore_snapshot_in_place(*expected) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                continue;
            }
            RegionOwnerUndo::Effect { lease, checkpoint } => {
                let store = &mut regions
                    .get_mut(&lease.key)
                    .ok_or(RegionOwnerLaneError::UnknownRegion)?
                    .1;
                if !store.restore_effect_checkpoint(*checkpoint) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                continue;
            }
            RegionOwnerUndo::Snapshot {
                lease,
                snapshot,
                allow_type_change,
            } => {
                let store = &mut regions
                    .get_mut(&lease.key)
                    .ok_or(RegionOwnerLaneError::UnknownRegion)?
                    .1;
                let restored = if allow_type_change {
                    store.convert_snapshot_in_place(*snapshot)
                } else {
                    store.restore_snapshot_in_place(*snapshot)
                };
                if !restored {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                continue;
            }
            RegionOwnerUndo::GoalBatch { lease, checkpoints } => {
                let store = &mut regions
                    .get_mut(&lease.key)
                    .ok_or(RegionOwnerLaneError::UnknownRegion)?
                    .1;
                if !store.restore_goal_checkpoints(checkpoints) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                continue;
            }
        };
        let store = &mut regions
            .get_mut(&lease.key)
            .ok_or(RegionOwnerLaneError::UnknownRegion)?
            .1;
        let restored = match restored {
            RegionOwnerMutation::SetVelocity { velocity, .. } => {
                store.set_velocity(entity, velocity)
            }
            RegionOwnerMutation::SetAnimalState { animal, .. } => {
                store.set_animal_state(entity, animal)
            }
            RegionOwnerMutation::SetAnimalStateIfCurrent { .. } => {
                unreachable!("conditional animal undo restores directly")
            }
            RegionOwnerMutation::SetGrazingStateIfCurrent { .. } => {
                unreachable!("conditional grazing undo restores directly")
            }
            RegionOwnerMutation::SetGoalIfCurrent { .. } => {
                unreachable!("conditional goal undo restores directly")
            }
            RegionOwnerMutation::SetItemStackIfCurrent { .. } => {
                unreachable!("conditional item stack undo restores directly")
            }
            RegionOwnerMutation::ReplaceSnapshotIfCurrent { .. } => {
                unreachable!("snapshot undo restores directly")
            }
            RegionOwnerMutation::SetKinematicsIfCurrent { .. } => {
                unreachable!("kinematics undo restores directly")
            }
            RegionOwnerMutation::SetKinematicsBatchIfCurrent { .. } => {
                unreachable!("kinematics batch undo restores directly")
            }
            RegionOwnerMutation::SetKinematicsBatchIfVersion { .. } => {
                unreachable!("versioned kinematics batch undo restores directly")
            }
            RegionOwnerMutation::DamageIfCurrent { .. } => {
                unreachable!("damage undo restores snapshot directly")
            }
            RegionOwnerMutation::ApplyEffectIfCurrent { .. } => {
                unreachable!("effect undo restores the ECS component checkpoint directly")
            }
            RegionOwnerMutation::ApplyGoalBatch { .. } => {
                unreachable!("goal undo restores kinematics directly")
            }
            RegionOwnerMutation::InsertSnapshot(_) => unreachable!("insert undo removes directly"),
            RegionOwnerMutation::InsertSnapshots(_) => {
                unreachable!("batch insert undo removes directly")
            }
            RegionOwnerMutation::RemoveEntity(_) => unreachable!("remove undo inserts directly"),
            RegionOwnerMutation::RemoveIfCurrent(_) => {
                unreachable!("conditional remove undo inserts directly")
            }
            RegionOwnerMutation::RemoveSnapshotsIfCurrent(_) => {
                unreachable!("conditional batch remove undo inserts directly")
            }
        };
        if !restored {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
    }
    Ok(())
}
