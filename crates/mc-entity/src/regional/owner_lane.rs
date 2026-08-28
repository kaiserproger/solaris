use std::collections::{BTreeMap, HashMap, HashSet};
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use super::{
    RegionKey, RegionLease, RegionPhase, add_goal_tick_stats, goal_reference,
    order_vehicle_group_for_removal, snapshot_vehicle_reference,
};

use crate::lock_policy::lock_authoritative_mutex;
use crate::{
    AnimalBreedingState, EntityDamageRequest, EntityEffectRequest, EntityEffectResult,
    EntityGoalCheckpoint, EntityId, EntityItemStack, EntityKinematics, EntitySimulationProjection,
    EntitySnapshot, EntityStore, GoalState, GoalTickStats, PreparedGoalTick, ResolvedGoalTick,
    Vec3,
};

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
        expected_state_version: u64,
        resolved: Box<ResolvedGoalTick>,
        follow_targets: HashMap<EntityId, Vec3>,
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

#[derive(Debug, Clone, PartialEq)]
pub struct RegionOwnerCompletion {
    pub phase: RegionPhase,
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
    GoalCheckpointsForIds {
        entities: Vec<(RegionLease, EntityId)>,
        reply: std::sync::mpsc::Sender<Result<Vec<EntityGoalCheckpoint>, RegionOwnerLaneError>>,
    },
    AliveKinematicsForIds {
        entities: Vec<(RegionLease, EntityId)>,
        reply: std::sync::mpsc::Sender<Result<Vec<EntityKinematics>, RegionOwnerLaneError>>,
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
        reply: std::sync::mpsc::Sender<Result<PreparedGoalTick, RegionOwnerLaneError>>,
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

#[derive(Clone)]
pub(super) struct RegionalOwnerLaneReader {
    sender: SyncSender<RegionOwnerLaneMessage>,
    health: Arc<RegionOwnerLaneHealth>,
    admission: Arc<Mutex<()>>,
    state_version: Arc<AtomicU64>,
}

impl RegionalOwnerLaneReader {
    fn unavailable_error(&self) -> RegionOwnerLaneError {
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

    pub(super) fn request_simulation_projections_for_ids(
        &self,
        entities: Vec<(RegionLease, EntityId)>,
    ) -> Result<
        Receiver<Result<Vec<EntitySimulationProjection>, RegionOwnerLaneError>>,
        RegionOwnerLaneError,
    > {
        let (reply, projections) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::SimulationProjectionsForIds { entities, reply })
            .map_err(|_| self.unavailable_error())?;
        Ok(projections)
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
    ) -> Result<Receiver<Result<Vec<EntityKinematics>, RegionOwnerLaneError>>, RegionOwnerLaneError>
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
            .try_send(RegionOwnerLaneMessage::SnapshotsForIds { entities, reply })
        {
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
    ) -> Result<Receiver<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>>, RegionOwnerLaneError>
    {
        #[cfg(test)]
        self.snapshot_batch_requests.fetch_add(1, Ordering::Relaxed);
        let (reply, snapshots) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::SnapshotsForIds { entities, reply })
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
    ) -> Result<Receiver<Result<PreparedGoalTick, RegionOwnerLaneError>>, RegionOwnerLaneError>
    {
        let (reply, prepared) = channel();
        self.sender
            .send(RegionOwnerLaneMessage::PrepareGoalTick {
                lease,
                tick,
                active_ids,
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
    ) -> Result<Receiver<Result<PreparedGoalTick, RegionOwnerLaneError>>, RegionOwnerLaneError>
    {
        let _admission = lock_authoritative_mutex(&self.admission, "regional.owner_lane_admission");
        self.request_goal_tick(lease, tick, active_ids)
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
            RegionOwnerLaneMessage::SnapshotsForIds { entities, reply } => {
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
                let mut projections = Vec::with_capacity(entities.len());
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
                        projections.extend(store.simulation_projections_for_ids(&ids));
                    }
                }
                projections.sort_unstable_by_key(|projection| projection.id);
                let _ = reply.send(match error {
                    Some(error) => Err(error),
                    None => Ok(projections),
                });
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
                let mut states = Vec::new();
                if error.is_none() {
                    for (key, ids) in ids_by_region {
                        states.extend(
                            regions
                                .get_mut(&key)
                                .expect("validated regional kinematics route")
                                .1
                                .alive_kinematics_for_ids(&ids),
                        );
                    }
                    states.sort_unstable_by_key(|state| state.id);
                }
                let _ = reply.send(match error {
                    Some(error) => Err(error),
                    None => Ok(states),
                });
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
                        Ok(store.prepare_goal_tick_with_pathing_for_ids(tick, &active_ids))
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
            RegionOwnerMutation::DamageIfCurrent { expected, request } => {
                let passengers = passengers_by_region
                    .entry(mutation.lease.key)
                    .or_insert_with(|| {
                        store
                            .snapshots()
                            .filter_map(|snapshot| snapshot_vehicle_reference(&snapshot))
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
                expected_state_version,
                resolved,
                ..
            } => {
                let expected_ids = expected
                    .iter()
                    .map(|checkpoint| checkpoint.id)
                    .collect::<HashSet<_>>();
                if *expected_state_version != current_state_version
                    || expected_ids.len() != expected.len()
                    || expected.iter().any(|checkpoint| {
                        RegionKey::from_position(checkpoint.position) != Some(mutation.lease.key)
                            || store.goal_checkpoint(checkpoint.id).as_ref() != Some(checkpoint)
                    })
                    || resolved
                        .active_ids
                        .as_ref()
                        .is_some_and(|active| *active != expected_ids)
                    || (resolved.active_ids.is_none() && expected_ids.len() != store.len())
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
                let applied = store.apply_kinematics(states) == expected_count;
                if applied {
                    undo.push(RegionOwnerUndo::KinematicsBatch {
                        lease: mutation.lease,
                        states: previous,
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
                expected_state_version: _,
                resolved,
                follow_targets,
            } => {
                let checkpoints = expected;
                let stats =
                    store.apply_prepared_goal_tick_with_follow_targets(*resolved, &follow_targets);
                add_goal_tick_stats(&mut goal_stats, stats);
                undo.push(RegionOwnerUndo::GoalBatch {
                    lease: mutation.lease,
                    checkpoints,
                });
                true
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
    let completion = RegionOwnerCompletion {
        phase: batch.phase,
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
