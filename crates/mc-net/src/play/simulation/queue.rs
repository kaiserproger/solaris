use super::{
    JavaLegacyRandom, PlayerPose, RegionOwnership, SessionId, SimulationAuthority,
    SimulationCommand, SimulationHandle, SimulationOutcome, SimulationOwner,
    SimulationRequestError, command_is_background, command_orders_earlier_herds,
};
use crate::play::session::ScriptPlayerTeleportCompletion;
use mc_script::ScriptPlayerTeleportFailure;
use std::collections::{HashMap, VecDeque};
#[cfg(test)]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use tokio::sync::{mpsc, oneshot};

pub(crate) const SIMULATION_COMMAND_QUEUE_CAPACITY: usize = 1024;
pub(crate) const SIMULATION_COMMAND_BATCH_LIMIT: usize = 256;
pub(super) const SIMULATION_BACKGROUND_COMMAND_BATCH_LIMIT: usize = 2;

#[derive(Debug)]
pub(super) struct SimulationCommandEnvelope {
    pub(super) sequence: u64,
    pub(super) command: SimulationCommand,
    pub(super) session_fence: Option<SessionId>,
    response: Option<oneshot::Sender<SimulationOutcome>>,
}

impl SimulationCommandEnvelope {
    pub(super) fn response_is_closed(&self) -> bool {
        self.response
            .as_ref()
            .is_some_and(oneshot::Sender::is_closed)
    }

    pub(super) fn is_detached(&self) -> bool {
        self.response.is_none()
    }

    pub(super) fn respond(mut self, outcome: SimulationOutcome) {
        self.command.complete_script_player_teleport(&outcome);
        if let Some(response) = self.response {
            let _ = response.send(outcome);
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SimulationQueueSnapshot {
    pub(crate) capacity: usize,
    pub(crate) depth: usize,
    pub(crate) max_depth: usize,
    pub(crate) enqueued: u64,
    pub(crate) dequeued: u64,
    pub(crate) processed: u64,
    pub(crate) item_pickups_processed: u64,
    pub(crate) block_edits_processed: u64,
    pub(crate) container_commits_processed: u64,
    pub(crate) block_entity_commits_processed: u64,
    pub(crate) rejected_full: u64,
    pub(crate) rejected_closed: u64,
    pub(crate) rejected_shutdown: u64,
    pub(crate) rejected_world_busy: u64,
    pub(crate) rejected_world_unavailable: u64,
    pub(crate) rejected_world_mutation: u64,
    pub(crate) rejected_stale_session: u64,
    pub(crate) cancelled: u64,
    pub(crate) max_batch: usize,
}

#[derive(Debug)]
pub(super) struct SimulationQueueMetrics {
    pub(super) capacity: usize,
    pub(super) next_sequence: AtomicU64,
    pub(super) depth: AtomicUsize,
    pub(super) max_depth: AtomicUsize,
    pub(super) enqueued: AtomicU64,
    pub(super) dequeued: AtomicU64,
    pub(super) processed: AtomicU64,
    pub(super) item_pickups_processed: AtomicU64,
    pub(super) block_edits_processed: AtomicU64,
    pub(super) container_commits_processed: AtomicU64,
    pub(super) block_entity_commits_processed: AtomicU64,
    pub(super) rejected_full: AtomicU64,
    pub(super) rejected_closed: AtomicU64,
    pub(super) rejected_shutdown: AtomicU64,
    pub(super) rejected_world_busy: AtomicU64,
    pub(super) rejected_world_unavailable: AtomicU64,
    pub(super) rejected_world_mutation: AtomicU64,
    pub(super) rejected_stale_session: AtomicU64,
    pub(super) cancelled: AtomicU64,
    pub(super) max_batch: AtomicUsize,
    requested_herd_chunks: Mutex<HashMap<(i32, i32), Arc<HerdEnqueueClaim>>>,
    #[cfg(test)]
    herd_enqueue_probe: Mutex<Option<Arc<HerdEnqueueProbe>>>,
}

#[derive(Debug)]
struct HerdEnqueueClaim {
    outcome: Mutex<Option<Result<(), SimulationRequestError>>>,
    completed: Condvar,
}

impl HerdEnqueueClaim {
    fn pending() -> Self {
        Self {
            outcome: Mutex::new(None),
            completed: Condvar::new(),
        }
    }
}

#[cfg(test)]
#[derive(Debug)]
struct HerdEnqueueProbe {
    winner_claimed: std::sync::mpsc::SyncSender<()>,
    release_winner: Mutex<std::sync::mpsc::Receiver<()>>,
    waiter_blocked: std::sync::mpsc::SyncSender<()>,
    winner_announced: AtomicBool,
    waiter_announced: AtomicBool,
}

#[cfg(test)]
impl HerdEnqueueProbe {
    fn pause_winner(&self) {
        if !self.winner_announced.swap(true, Ordering::AcqRel) {
            self.winner_claimed
                .send(())
                .expect("herd enqueue winner probe receiver");
            self.release_winner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .recv()
                .expect("herd enqueue winner probe release");
        }
    }

    fn notify_waiter(&self) {
        if !self.waiter_announced.swap(true, Ordering::AcqRel) {
            self.waiter_blocked
                .send(())
                .expect("herd enqueue waiter probe receiver");
        }
    }
}

impl SimulationQueueMetrics {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            next_sequence: AtomicU64::new(0),
            depth: AtomicUsize::new(0),
            max_depth: AtomicUsize::new(0),
            enqueued: AtomicU64::new(0),
            dequeued: AtomicU64::new(0),
            processed: AtomicU64::new(0),
            item_pickups_processed: AtomicU64::new(0),
            block_edits_processed: AtomicU64::new(0),
            container_commits_processed: AtomicU64::new(0),
            block_entity_commits_processed: AtomicU64::new(0),
            rejected_full: AtomicU64::new(0),
            rejected_closed: AtomicU64::new(0),
            rejected_shutdown: AtomicU64::new(0),
            rejected_world_busy: AtomicU64::new(0),
            rejected_world_unavailable: AtomicU64::new(0),
            rejected_world_mutation: AtomicU64::new(0),
            rejected_stale_session: AtomicU64::new(0),
            cancelled: AtomicU64::new(0),
            max_batch: AtomicUsize::new(0),
            requested_herd_chunks: Mutex::new(HashMap::new()),
            #[cfg(test)]
            herd_enqueue_probe: Mutex::new(None),
        }
    }

    fn snapshot(&self) -> SimulationQueueSnapshot {
        SimulationQueueSnapshot {
            capacity: self.capacity,
            depth: self.depth.load(Ordering::Relaxed),
            max_depth: self.max_depth.load(Ordering::Relaxed),
            enqueued: self.enqueued.load(Ordering::Relaxed),
            dequeued: self.dequeued.load(Ordering::Relaxed),
            processed: self.processed.load(Ordering::Relaxed),
            item_pickups_processed: self.item_pickups_processed.load(Ordering::Relaxed),
            block_edits_processed: self.block_edits_processed.load(Ordering::Relaxed),
            container_commits_processed: self.container_commits_processed.load(Ordering::Relaxed),
            block_entity_commits_processed: self
                .block_entity_commits_processed
                .load(Ordering::Relaxed),
            rejected_full: self.rejected_full.load(Ordering::Relaxed),
            rejected_closed: self.rejected_closed.load(Ordering::Relaxed),
            rejected_shutdown: self.rejected_shutdown.load(Ordering::Relaxed),
            rejected_world_busy: self.rejected_world_busy.load(Ordering::Relaxed),
            rejected_world_unavailable: self.rejected_world_unavailable.load(Ordering::Relaxed),
            rejected_world_mutation: self.rejected_world_mutation.load(Ordering::Relaxed),
            rejected_stale_session: self.rejected_stale_session.load(Ordering::Relaxed),
            cancelled: self.cancelled.load(Ordering::Relaxed),
            max_batch: self.max_batch.load(Ordering::Relaxed),
        }
    }

    fn record_depth(&self, depth: usize) {
        record_atomic_max(&self.max_depth, depth);
    }

    pub(super) fn record_batch(&self, batch: usize) {
        record_atomic_max(&self.max_batch, batch);
    }
}

fn record_atomic_max(target: &AtomicUsize, candidate: usize) {
    let mut observed = target.load(Ordering::Relaxed);
    while candidate > observed {
        match target.compare_exchange_weak(
            observed,
            candidate,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => return,
            Err(actual) => observed = actual,
        }
    }
}

impl SimulationHandle {
    #[cfg(test)]
    pub(super) fn install_herd_enqueue_probe_for_test(
        &self,
        winner_claimed: std::sync::mpsc::SyncSender<()>,
        release_winner: std::sync::mpsc::Receiver<()>,
        waiter_blocked: std::sync::mpsc::SyncSender<()>,
    ) {
        *self
            .metrics
            .herd_enqueue_probe
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::new(HerdEnqueueProbe {
            winner_claimed,
            release_winner: Mutex::new(release_winner),
            waiter_blocked,
            winner_announced: AtomicBool::new(false),
            waiter_announced: AtomicBool::new(false),
        }));
    }

    pub(in super::super) fn for_session(&self, session_id: SessionId) -> Self {
        Self {
            sender: self.sender.clone(),
            metrics: Arc::clone(&self.metrics),
            session_fence: Some(session_id),
        }
    }

    #[cfg(test)]
    pub(in super::super) fn enqueue(
        &self,
        command: SimulationCommand,
    ) -> Result<oneshot::Receiver<SimulationOutcome>, SimulationRequestError> {
        self.enqueue_with_fence(self.session_fence, command)
    }

    pub(super) fn enqueue_player_command(
        &self,
        command: SimulationCommand,
    ) -> Result<oneshot::Receiver<SimulationOutcome>, SimulationRequestError> {
        let session_id = self
            .session_fence
            .ok_or(SimulationRequestError::InvalidCommand)?;
        self.enqueue_with_fence(Some(session_id), command)
    }

    pub(super) async fn enqueue_player_command_wait(
        &self,
        command: SimulationCommand,
    ) -> Result<oneshot::Receiver<SimulationOutcome>, SimulationRequestError> {
        let session_id = self
            .session_fence
            .ok_or(SimulationRequestError::InvalidCommand)?;
        let sequence = self.metrics.next_sequence.fetch_add(1, Ordering::Relaxed);
        let (response, receiver) = oneshot::channel();
        let permit = self.sender.reserve().await.map_err(|_| {
            self.metrics.rejected_closed.fetch_add(1, Ordering::Relaxed);
            SimulationRequestError::Closed
        })?;
        self.metrics.enqueued.fetch_add(1, Ordering::Relaxed);
        let depth = self.metrics.depth.fetch_add(1, Ordering::Relaxed) + 1;
        self.metrics.record_depth(depth);
        permit.send(SimulationCommandEnvelope {
            sequence,
            command,
            session_fence: Some(session_id),
            response: Some(response),
        });
        Ok(receiver)
    }

    pub(super) async fn enqueue_script_player_teleport_wait(
        &self,
        pose: PlayerPose,
        completion: ScriptPlayerTeleportCompletion,
    ) -> Result<oneshot::Receiver<SimulationOutcome>, SimulationRequestError> {
        let Some(session_id) = self.session_fence else {
            completion.complete(Err(ScriptPlayerTeleportFailure::RuntimeUnavailable));
            return Err(SimulationRequestError::InvalidCommand);
        };
        let sequence = self.metrics.next_sequence.fetch_add(1, Ordering::Relaxed);
        let (response, receiver) = oneshot::channel();
        let permit = match self.sender.reserve().await {
            Ok(permit) => permit,
            Err(_) => {
                self.metrics.rejected_closed.fetch_add(1, Ordering::Relaxed);
                completion.complete(Err(ScriptPlayerTeleportFailure::RuntimeUnavailable));
                return Err(SimulationRequestError::Closed);
            }
        };
        self.metrics.enqueued.fetch_add(1, Ordering::Relaxed);
        let depth = self.metrics.depth.fetch_add(1, Ordering::Relaxed) + 1;
        self.metrics.record_depth(depth);
        permit.send(SimulationCommandEnvelope {
            sequence,
            command: SimulationCommand::CommitPlayerPose {
                actor_session: session_id,
                pose,
                exhaustion: 0.0,
                script_teleport_completion: Some(completion),
            },
            session_fence: Some(session_id),
            response: Some(response),
        });
        Ok(receiver)
    }

    pub(super) fn session_id(&self) -> Result<SessionId, SimulationRequestError> {
        self.session_fence
            .ok_or(SimulationRequestError::InvalidCommand)
    }

    pub(super) fn enqueue_with_fence(
        &self,
        session_fence: Option<SessionId>,
        command: SimulationCommand,
    ) -> Result<oneshot::Receiver<SimulationOutcome>, SimulationRequestError> {
        let sequence = self.metrics.next_sequence.fetch_add(1, Ordering::Relaxed);
        let (response, receiver) = oneshot::channel();
        let envelope = SimulationCommandEnvelope {
            sequence,
            command,
            session_fence,
            response: Some(response),
        };
        self.try_send(envelope)?;
        Ok(receiver)
    }

    fn enqueue_detached(&self, command: SimulationCommand) -> Result<(), SimulationRequestError> {
        let sequence = self.metrics.next_sequence.fetch_add(1, Ordering::Relaxed);
        self.try_send(SimulationCommandEnvelope {
            sequence,
            command,
            session_fence: None,
            response: None,
        })
    }

    pub(super) async fn enqueue_detached_wait(
        &self,
        command: SimulationCommand,
    ) -> Result<(), SimulationRequestError> {
        let sequence = self.metrics.next_sequence.fetch_add(1, Ordering::Relaxed);
        let permit = self.sender.reserve().await.map_err(|_| {
            self.metrics.rejected_closed.fetch_add(1, Ordering::Relaxed);
            SimulationRequestError::Closed
        })?;
        self.metrics.enqueued.fetch_add(1, Ordering::Relaxed);
        let depth = self.metrics.depth.fetch_add(1, Ordering::Relaxed) + 1;
        self.metrics.record_depth(depth);
        permit.send(SimulationCommandEnvelope {
            sequence,
            command,
            session_fence: None,
            response: None,
        });
        Ok(())
    }

    fn try_send(&self, envelope: SimulationCommandEnvelope) -> Result<(), SimulationRequestError> {
        match self.sender.try_reserve() {
            Ok(permit) => {
                self.metrics.enqueued.fetch_add(1, Ordering::Relaxed);
                let depth = self.metrics.depth.fetch_add(1, Ordering::Relaxed) + 1;
                self.metrics.record_depth(depth);
                permit.send(envelope);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.metrics.rejected_full.fetch_add(1, Ordering::Relaxed);
                Err(SimulationRequestError::Full)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.metrics.rejected_closed.fetch_add(1, Ordering::Relaxed);
                Err(SimulationRequestError::Closed)
            }
        }
    }

    pub(in super::super) fn ensure_chunk_herd(
        &self,
        chunk: (i32, i32),
        spawns: Vec<super::HerdSpawn>,
    ) -> Result<(), SimulationRequestError> {
        let (claim, winner) = {
            let mut requested = self
                .metrics
                .requested_herd_chunks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(claim) = requested.get(&chunk) {
                (Arc::clone(claim), false)
            } else {
                let claim = Arc::new(HerdEnqueueClaim::pending());
                requested.insert(chunk, Arc::clone(&claim));
                (claim, true)
            }
        };
        if !winner {
            #[cfg(test)]
            if let Some(probe) = {
                self.metrics
                    .herd_enqueue_probe
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone()
            } {
                probe.notify_waiter();
            }
            let mut outcome = claim
                .outcome
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            while outcome.is_none() {
                outcome = claim
                    .completed
                    .wait(outcome)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            return outcome.expect("completed herd enqueue claim has an outcome");
        }

        #[cfg(test)]
        if let Some(probe) = {
            self.metrics
                .herd_enqueue_probe
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        } {
            probe.pause_winner();
        }
        let result = self.enqueue_detached(SimulationCommand::EnsureChunkHerd { chunk, spawns });
        if result.is_err() {
            let mut requested = self
                .metrics
                .requested_herd_chunks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if requested
                .get(&chunk)
                .is_some_and(|current| Arc::ptr_eq(current, &claim))
            {
                requested.remove(&chunk);
            }
        }
        *claim
            .outcome
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(result);
        claim.completed.notify_all();
        result
    }

    pub(crate) fn snapshot(&self) -> super::SimulationQueueSnapshot {
        self.metrics.snapshot()
    }
}

impl SimulationOwner {
    pub(crate) async fn wait_for_command(&mut self) -> bool {
        if self.prefetched.is_some() {
            return true;
        }
        let Some(envelope) = self.receiver.recv().await else {
            return false;
        };
        self.metrics.dequeued.fetch_add(1, Ordering::Relaxed);
        self.prefetched = Some(envelope);
        true
    }

    pub(super) fn release_retryable_herd_requests(&self, chunks: &[(i32, i32)]) {
        if chunks.is_empty() {
            return;
        }
        let mut requested = self
            .metrics
            .requested_herd_chunks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for chunk in chunks {
            requested.remove(chunk);
        }
    }

    fn take_queued_command(&mut self) -> Option<SimulationCommandEnvelope> {
        if let Some(envelope) = self.prefetched.take() {
            return Some(envelope);
        }
        match self.receiver.try_recv() {
            Ok(envelope) => {
                self.metrics.dequeued.fetch_add(1, Ordering::Relaxed);
                Some(envelope)
            }
            Err(mpsc::error::TryRecvError::Empty | mpsc::error::TryRecvError::Disconnected) => None,
        }
    }

    fn collect_ready_batch(&mut self, budget: usize) -> Vec<SimulationCommandEnvelope> {
        let mut batch = Vec::with_capacity(budget.min(self.metrics.capacity));
        let mut scanned = 0usize;
        let mut background_admitted = 0usize;
        while batch.len() < budget && scanned < self.metrics.capacity {
            let Some(envelope) = self.take_queued_command() else {
                break;
            };
            scanned += 1;
            if envelope.response_is_closed() {
                self.metrics.depth.fetch_sub(1, Ordering::Relaxed);
                self.metrics.cancelled.fetch_add(1, Ordering::Relaxed);
            } else if command_is_background(&envelope.command) {
                self.deferred_background.push_back(envelope);
            } else {
                if command_orders_earlier_herds(&envelope.command) {
                    while background_admitted < SIMULATION_BACKGROUND_COMMAND_BATCH_LIMIT
                        && batch.len() < budget
                        && self
                            .deferred_background
                            .front()
                            .is_some_and(|deferred| deferred.sequence < envelope.sequence)
                    {
                        let deferred = self
                            .deferred_background
                            .pop_front()
                            .expect("matching earlier herd command");
                        self.metrics.depth.fetch_sub(1, Ordering::Relaxed);
                        batch.push(deferred);
                        background_admitted += 1;
                    }
                    if batch.len() == budget
                        || self
                            .deferred_background
                            .front()
                            .is_some_and(|deferred| deferred.sequence < envelope.sequence)
                    {
                        self.prefetched = Some(envelope);
                        break;
                    }
                }
                self.metrics.depth.fetch_sub(1, Ordering::Relaxed);
                batch.push(envelope);
            }
        }
        batch.sort_unstable_by_key(|envelope| envelope.sequence);
        batch
    }

    pub(super) fn drain_ready_batch(&mut self, budget: usize) -> Vec<SimulationCommandEnvelope> {
        let batch = self.collect_ready_batch(budget);
        self.metrics.record_batch(batch.len());
        batch
    }

    pub(super) fn drain_batch(&mut self, budget: usize) -> Vec<SimulationCommandEnvelope> {
        let mut batch = self.collect_ready_batch(budget);
        let background_admitted = batch
            .iter()
            .filter(|envelope| command_is_background(&envelope.command))
            .count();
        let background_budget = budget
            .saturating_sub(batch.len())
            .min(SIMULATION_BACKGROUND_COMMAND_BATCH_LIMIT.saturating_sub(background_admitted));
        for _ in 0..background_budget {
            let Some(envelope) = self.deferred_background.pop_front() else {
                break;
            };
            self.metrics.depth.fetch_sub(1, Ordering::Relaxed);
            batch.push(envelope);
        }
        self.metrics.record_batch(batch.len());
        batch
    }

    pub(crate) fn shutdown(&mut self) {
        self.reject_pending(SimulationRequestError::ShuttingDown);
    }

    fn reject_pending(&mut self, error: SimulationRequestError) {
        self.receiver.close();
        self.metrics
            .requested_herd_chunks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        if let Some(envelope) = self.prefetched.take() {
            self.metrics.depth.fetch_sub(1, Ordering::Relaxed);
            if envelope.response_is_closed() {
                self.metrics.cancelled.fetch_add(1, Ordering::Relaxed);
            } else {
                self.metrics
                    .rejected_shutdown
                    .fetch_add(1, Ordering::Relaxed);
                envelope.respond(Err(error));
            }
        }
        while let Some(envelope) = self.deferred_background.pop_front() {
            self.metrics.depth.fetch_sub(1, Ordering::Relaxed);
            if envelope.response_is_closed() {
                self.metrics.cancelled.fetch_add(1, Ordering::Relaxed);
            } else {
                self.metrics
                    .rejected_shutdown
                    .fetch_add(1, Ordering::Relaxed);
                envelope.respond(Err(error));
            }
        }
        while let Ok(envelope) = self.receiver.try_recv() {
            self.metrics.depth.fetch_sub(1, Ordering::Relaxed);
            self.metrics.dequeued.fetch_add(1, Ordering::Relaxed);
            if envelope.response_is_closed() {
                self.metrics.cancelled.fetch_add(1, Ordering::Relaxed);
            } else {
                self.metrics
                    .rejected_shutdown
                    .fetch_add(1, Ordering::Relaxed);
                envelope.respond(Err(error));
            }
        }
    }
}

impl Drop for SimulationOwner {
    fn drop(&mut self) {
        self.reject_pending(SimulationRequestError::OwnerStopped);
    }
}

#[cfg(test)]
pub(crate) fn simulation_channel() -> (SimulationHandle, SimulationOwner) {
    simulation_channel_with_explosion_seed(0)
}

pub(crate) fn simulation_channel_with_explosion_seed(
    explosion_seed: i64,
) -> (SimulationHandle, SimulationOwner) {
    simulation_channel_with_capacity_and_explosion_seed(
        SIMULATION_COMMAND_QUEUE_CAPACITY,
        explosion_seed,
    )
}

#[cfg(test)]
pub(in super::super) fn simulation_channel_with_capacity(
    capacity: usize,
) -> (SimulationHandle, SimulationOwner) {
    simulation_channel_with_capacity_and_explosion_seed(capacity, 0)
}

fn simulation_channel_with_capacity_and_explosion_seed(
    capacity: usize,
    explosion_seed: i64,
) -> (SimulationHandle, SimulationOwner) {
    assert!(capacity > 0, "simulation command capacity must be positive");
    let (sender, receiver) = mpsc::channel(capacity);
    let metrics = Arc::new(SimulationQueueMetrics::new(capacity));
    (
        SimulationHandle {
            sender,
            metrics: Arc::clone(&metrics),
            session_fence: None,
        },
        SimulationOwner {
            receiver,
            prefetched: None,
            deferred_background: VecDeque::new(),
            metrics,
            authority: SimulationAuthority(()),
            region_ownership: RegionOwnership::new(),
            explosion_random: JavaLegacyRandom::new(explosion_seed),
            #[cfg(test)]
            last_region_routes: Vec::new(),
            #[cfg(test)]
            regional_block_edit_probe: None,
        },
    )
}
