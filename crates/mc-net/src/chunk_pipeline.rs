//! Chunk-pipeline policy and hand-off types.
//!
//! M13 starts by naming the scheduler/worker boundary before moving work
//! across it. The policy is runtime configuration; the request/result
//! types describe the ownership we want between Play socket tasks and the
//! bounded chunk workers.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::{AcquireError, Notify, OwnedSemaphorePermit, Semaphore};

use crate::control_plane::RuntimeControlConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkPipelinePolicy {
    pub chunk_send_rate: u32,
    pub chunk_load_rate: u32,
    pub chunk_generate_rate: u32,
    pub chunk_prepare_budget_ms: u64,
    pub chunk_prepare_batch_size: usize,
    pub chunk_io_threads: usize,
    /// Shared CPU capacity for chunk work and entity physics.
    pub chunk_worker_threads: usize,
    pub chunk_result_queue_size: usize,
    pub region_cache_size: usize,
    pub compression_threshold: i32,
    pub compression_level: Option<u32>,
    pub runtime_control: Option<RuntimeControlConfig>,
}

impl Default for ChunkPipelinePolicy {
    fn default() -> Self {
        let available = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1);
        let (chunk_io_threads, cpu_worker_threads) = automatic_worker_limits(available);
        Self {
            chunk_send_rate: 16,
            chunk_load_rate: 64,
            chunk_generate_rate: 32,
            chunk_prepare_budget_ms: 0,
            chunk_prepare_batch_size: 8,
            chunk_io_threads,
            chunk_worker_threads: cpu_worker_threads,
            chunk_result_queue_size: 64,
            region_cache_size: 4,
            compression_threshold: crate::login::LOGIN_COMPRESSION_THRESHOLD,
            compression_level: None,
            runtime_control: None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ChunkPipelineResources {
    io_permits: Arc<Semaphore>,
    cpu_permits: Arc<Semaphore>,
    prepare_request_permits: Arc<Semaphore>,
    cpu_capacity: usize,
    cpu_limit: Arc<AtomicUsize>,
    cpu_admission_changed: Arc<Notify>,
    prepare_admission_changed: Arc<Notify>,
    active_prepare_tasks: Arc<AtomicUsize>,
    active_prepare_requests: Arc<AtomicUsize>,
    metrics: ChunkPipelineResourceMetrics,
}

#[derive(Debug, Clone)]
pub struct ChunkPipelineIdleHandle {
    resources: ChunkPipelineResources,
}

impl ChunkPipelineIdleHandle {
    pub(crate) fn new(resources: ChunkPipelineResources) -> Self {
        Self { resources }
    }

    pub async fn wait_for_idle(&self) -> ChunkPipelineResourceSnapshot {
        self.resources.wait_for_idle().await;
        self.resources.metrics().snapshot()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChunkPipelineResourceMetrics {
    active_io: Arc<AtomicUsize>,
    max_io_active: Arc<AtomicUsize>,
    active_cpu: Arc<AtomicUsize>,
    max_cpu_active: Arc<AtomicUsize>,
    max_result_queue_depth: Arc<AtomicUsize>,
    stop_reasons: Arc<ChunkPipelineStopReasonMetrics>,
    cancellations: Arc<ChunkPipelineCancellationMetrics>,
    idle_changed: Arc<Notify>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkPipelineResourceSnapshot {
    pub active_io: usize,
    pub max_io_active: usize,
    pub active_cpu: usize,
    pub max_cpu_active: usize,
    pub max_result_queue_depth: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChunkPipelineCancellationSnapshot {
    pub cancelled_streams: usize,
    pub cancelled_requests: usize,
    pub stale_results_rejected: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChunkPipelineStopReasonCounts {
    pub batch_limit: usize,
    pub time_budget: usize,
    pub send_budget: usize,
    pub load_budget: usize,
    pub generate_budget: usize,
    pub memory_pressure: usize,
    pub queue_full: usize,
    pub queue_empty: usize,
    pub complete: usize,
}

#[derive(Debug, Default)]
struct ChunkPipelineStopReasonMetrics {
    batch_limit: AtomicUsize,
    time_budget: AtomicUsize,
    send_budget: AtomicUsize,
    load_budget: AtomicUsize,
    generate_budget: AtomicUsize,
    memory_pressure: AtomicUsize,
    queue_full: AtomicUsize,
    queue_empty: AtomicUsize,
    complete: AtomicUsize,
}

#[derive(Debug, Default)]
struct ChunkPipelineCancellationMetrics {
    cancelled_streams: AtomicUsize,
    cancelled_requests: AtomicUsize,
    stale_results_rejected: AtomicUsize,
}

pub(crate) struct ChunkPipelinePermit {
    _permit: OwnedSemaphorePermit,
    active: Arc<AtomicUsize>,
    idle_changed: Arc<Notify>,
    admission_changed: Option<Arc<Notify>>,
}

pub(crate) struct ChunkPipelinePrepareTask {
    active: Arc<AtomicUsize>,
    idle_changed: Arc<Notify>,
}

impl Drop for ChunkPipelinePermit {
    fn drop(&mut self) {
        let previous = self.active.fetch_sub(1, Ordering::AcqRel);
        if let Some(changed) = self.admission_changed.as_ref() {
            changed.notify_one();
        }
        if previous == 1 {
            self.idle_changed.notify_waiters();
        }
    }
}

impl Drop for ChunkPipelinePrepareTask {
    fn drop(&mut self) {
        if self.active.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.idle_changed.notify_waiters();
        }
    }
}

impl ChunkPipelineResourceMetrics {
    #[must_use]
    pub fn snapshot(&self) -> ChunkPipelineResourceSnapshot {
        ChunkPipelineResourceSnapshot {
            active_io: self.active_io.load(Ordering::Acquire),
            max_io_active: self.max_io_active.load(Ordering::Acquire),
            active_cpu: self.active_cpu.load(Ordering::Acquire),
            max_cpu_active: self.max_cpu_active.load(Ordering::Acquire),
            max_result_queue_depth: self.max_result_queue_depth.load(Ordering::Acquire),
        }
    }

    #[must_use]
    pub fn stop_reason_counts(&self) -> ChunkPipelineStopReasonCounts {
        self.stop_reasons.snapshot()
    }

    #[must_use]
    pub fn observed_stop_reasons(&self) -> Vec<ChunkPipelineStopReason> {
        self.stop_reason_counts().observed_reasons()
    }

    #[must_use]
    pub fn cancellation_snapshot(&self) -> ChunkPipelineCancellationSnapshot {
        ChunkPipelineCancellationSnapshot {
            cancelled_streams: self.cancellations.cancelled_streams.load(Ordering::Acquire),
            cancelled_requests: self
                .cancellations
                .cancelled_requests
                .load(Ordering::Acquire),
            stale_results_rejected: self
                .cancellations
                .stale_results_rejected
                .load(Ordering::Acquire),
        }
    }

    pub(crate) fn record_stop_reason(&self, reason: ChunkPipelineStopReason) {
        self.stop_reasons
            .counter(reason)
            .fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn observe_result_queue_depth(&self, depth: usize) {
        self.max_result_queue_depth
            .fetch_max(depth, Ordering::AcqRel);
    }

    pub(crate) fn record_stream_cancellation(&self, requests: usize) {
        if requests == 0 {
            return;
        }
        self.cancellations
            .cancelled_streams
            .fetch_add(1, Ordering::AcqRel);
        self.cancellations
            .cancelled_requests
            .fetch_add(requests, Ordering::AcqRel);
    }

    pub(crate) fn record_stale_result_rejection(&self) {
        self.cancellations
            .stale_results_rejected
            .fetch_add(1, Ordering::AcqRel);
    }
}

impl ChunkPipelineStopReasonCounts {
    #[must_use]
    pub fn count(self, reason: ChunkPipelineStopReason) -> usize {
        match reason {
            ChunkPipelineStopReason::BatchLimit => self.batch_limit,
            ChunkPipelineStopReason::TimeBudget => self.time_budget,
            ChunkPipelineStopReason::SendBudget => self.send_budget,
            ChunkPipelineStopReason::LoadBudget => self.load_budget,
            ChunkPipelineStopReason::GenerateBudget => self.generate_budget,
            ChunkPipelineStopReason::MemoryPressure => self.memory_pressure,
            ChunkPipelineStopReason::QueueFull => self.queue_full,
            ChunkPipelineStopReason::QueueEmpty => self.queue_empty,
            ChunkPipelineStopReason::Complete => self.complete,
        }
    }

    #[must_use]
    pub fn observed_reasons(self) -> Vec<ChunkPipelineStopReason> {
        [
            ChunkPipelineStopReason::BatchLimit,
            ChunkPipelineStopReason::TimeBudget,
            ChunkPipelineStopReason::SendBudget,
            ChunkPipelineStopReason::LoadBudget,
            ChunkPipelineStopReason::GenerateBudget,
            ChunkPipelineStopReason::MemoryPressure,
            ChunkPipelineStopReason::QueueFull,
            ChunkPipelineStopReason::QueueEmpty,
            ChunkPipelineStopReason::Complete,
        ]
        .into_iter()
        .filter(|reason| self.count(*reason) > 0)
        .collect()
    }
}

impl ChunkPipelineStopReasonMetrics {
    fn snapshot(&self) -> ChunkPipelineStopReasonCounts {
        ChunkPipelineStopReasonCounts {
            batch_limit: self.batch_limit.load(Ordering::Acquire),
            time_budget: self.time_budget.load(Ordering::Acquire),
            send_budget: self.send_budget.load(Ordering::Acquire),
            load_budget: self.load_budget.load(Ordering::Acquire),
            generate_budget: self.generate_budget.load(Ordering::Acquire),
            memory_pressure: self.memory_pressure.load(Ordering::Acquire),
            queue_full: self.queue_full.load(Ordering::Acquire),
            queue_empty: self.queue_empty.load(Ordering::Acquire),
            complete: self.complete.load(Ordering::Acquire),
        }
    }

    fn counter(&self, reason: ChunkPipelineStopReason) -> &AtomicUsize {
        match reason {
            ChunkPipelineStopReason::BatchLimit => &self.batch_limit,
            ChunkPipelineStopReason::TimeBudget => &self.time_budget,
            ChunkPipelineStopReason::SendBudget => &self.send_budget,
            ChunkPipelineStopReason::LoadBudget => &self.load_budget,
            ChunkPipelineStopReason::GenerateBudget => &self.generate_budget,
            ChunkPipelineStopReason::MemoryPressure => &self.memory_pressure,
            ChunkPipelineStopReason::QueueFull => &self.queue_full,
            ChunkPipelineStopReason::QueueEmpty => &self.queue_empty,
            ChunkPipelineStopReason::Complete => &self.complete,
        }
    }
}

impl ChunkPipelineResources {
    #[must_use]
    pub(crate) fn new(policy: ChunkPipelinePolicy) -> Self {
        Self::with_limits(policy.chunk_io_threads, policy.chunk_worker_threads)
    }

    #[must_use]
    pub(crate) fn with_limits(chunk_io_threads: usize, cpu_worker_threads: usize) -> Self {
        let cpu_capacity = cpu_worker_threads.max(1);
        Self {
            io_permits: Arc::new(Semaphore::new(chunk_io_threads.max(1))),
            cpu_permits: Arc::new(Semaphore::new(cpu_capacity)),
            prepare_request_permits: Arc::new(Semaphore::new(cpu_capacity)),
            cpu_capacity,
            cpu_limit: Arc::new(AtomicUsize::new(cpu_capacity)),
            cpu_admission_changed: Arc::new(Notify::new()),
            prepare_admission_changed: Arc::new(Notify::new()),
            active_prepare_tasks: Arc::new(AtomicUsize::new(0)),
            active_prepare_requests: Arc::new(AtomicUsize::new(0)),
            metrics: ChunkPipelineResourceMetrics::default(),
        }
    }

    #[must_use]
    pub(crate) fn cpu_limit(&self) -> usize {
        self.cpu_limit.load(Ordering::Acquire)
    }

    pub(crate) fn apply_runtime_control_action(
        &self,
        action: crate::AutoscaleAction,
        draining: bool,
    ) -> usize {
        let current = self.cpu_limit();
        let next = if draining {
            1
        } else {
            match action {
                crate::AutoscaleAction::Hold => current,
                crate::AutoscaleAction::ScaleDown => current.div_ceil(2).max(1),
                crate::AutoscaleAction::ScaleUp => current.saturating_mul(2).min(self.cpu_capacity),
            }
        };
        if self.cpu_limit.swap(next, Ordering::AcqRel) != next {
            self.cpu_admission_changed.notify_waiters();
            self.prepare_admission_changed.notify_waiters();
        }
        next
    }

    #[must_use]
    pub(crate) fn metrics(&self) -> ChunkPipelineResourceMetrics {
        self.metrics.clone()
    }

    pub(crate) async fn wait_for_idle(&self) {
        loop {
            let idle_changed = self.metrics.idle_changed.notified();
            tokio::pin!(idle_changed);
            idle_changed.as_mut().enable();
            let snapshot = self.metrics.snapshot();
            if snapshot.active_io == 0
                && snapshot.active_cpu == 0
                && self.active_prepare_tasks.load(Ordering::Acquire) == 0
                && self.active_prepare_requests.load(Ordering::Acquire) == 0
            {
                return;
            }
            idle_changed.await;
        }
    }

    pub(crate) fn begin_prepare_task(&self) -> ChunkPipelinePrepareTask {
        self.active_prepare_tasks.fetch_add(1, Ordering::AcqRel);
        ChunkPipelinePrepareTask {
            active: Arc::clone(&self.active_prepare_tasks),
            idle_changed: Arc::clone(&self.metrics.idle_changed),
        }
    }

    pub(crate) fn record_stop_reason(&self, reason: ChunkPipelineStopReason) {
        self.metrics.record_stop_reason(reason);
    }

    pub(crate) fn observe_result_queue_depth(&self, depth: usize) {
        self.metrics.observe_result_queue_depth(depth);
    }

    pub(crate) fn record_stream_cancellation(&self, requests: usize) {
        self.metrics.record_stream_cancellation(requests);
    }

    pub(crate) fn record_stale_result_rejection(&self) {
        self.metrics.record_stale_result_rejection();
    }

    pub(crate) async fn acquire_io(&self) -> Result<ChunkPipelinePermit, AcquireError> {
        let permit = Arc::clone(&self.io_permits).acquire_owned().await?;
        Ok(self.track_io_permit(permit))
    }

    pub(crate) async fn acquire_cpu(&self) -> Result<ChunkPipelinePermit, AcquireError> {
        let permit = Arc::clone(&self.cpu_permits).acquire_owned().await?;
        loop {
            let changed = self.cpu_admission_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.try_reserve_cpu_slot() {
                return Ok(self.track_reserved_cpu_permit(permit));
            }
            changed.await;
        }
    }

    pub(crate) async fn acquire_prepare_request(
        &self,
    ) -> Result<ChunkPipelinePermit, AcquireError> {
        let permit = Arc::clone(&self.prepare_request_permits)
            .acquire_owned()
            .await?;
        loop {
            let changed = self.prepare_admission_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self
                .try_reserve_limited_slot(&self.active_prepare_requests)
                .is_some()
            {
                return Ok(ChunkPipelinePermit {
                    _permit: permit,
                    active: Arc::clone(&self.active_prepare_requests),
                    idle_changed: Arc::clone(&self.metrics.idle_changed),
                    admission_changed: Some(Arc::clone(&self.prepare_admission_changed)),
                });
            }
            changed.await;
        }
    }

    pub(crate) fn try_acquire_cpu(&self) -> Option<ChunkPipelinePermit> {
        let permit = Arc::clone(&self.cpu_permits).try_acquire_owned().ok()?;
        if !self.try_reserve_cpu_slot() {
            return None;
        }
        Some(self.track_reserved_cpu_permit(permit))
    }

    #[cfg(test)]
    pub(crate) fn try_acquire_prepare_request(&self) -> Option<ChunkPipelinePermit> {
        let permit = Arc::clone(&self.prepare_request_permits)
            .try_acquire_owned()
            .ok()?;
        self.try_reserve_limited_slot(&self.active_prepare_requests)?;
        Some(ChunkPipelinePermit {
            _permit: permit,
            active: Arc::clone(&self.active_prepare_requests),
            idle_changed: Arc::clone(&self.metrics.idle_changed),
            admission_changed: Some(Arc::clone(&self.prepare_admission_changed)),
        })
    }

    fn track_io_permit(&self, permit: OwnedSemaphorePermit) -> ChunkPipelinePermit {
        let active = &self.metrics.active_io;
        let now = active.fetch_add(1, Ordering::AcqRel) + 1;
        self.metrics.max_io_active.fetch_max(now, Ordering::AcqRel);
        ChunkPipelinePermit {
            _permit: permit,
            active: Arc::clone(active),
            idle_changed: Arc::clone(&self.metrics.idle_changed),
            admission_changed: None,
        }
    }

    fn try_reserve_cpu_slot(&self) -> bool {
        let Some(active) = self.try_reserve_limited_slot(&self.metrics.active_cpu) else {
            return false;
        };
        self.metrics
            .max_cpu_active
            .fetch_max(active, Ordering::AcqRel);
        true
    }

    fn try_reserve_limited_slot(&self, active_slots: &AtomicUsize) -> Option<usize> {
        let mut active = active_slots.load(Ordering::Acquire);
        loop {
            if active >= self.cpu_limit() {
                return None;
            }
            match active_slots.compare_exchange_weak(
                active,
                active + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(active + 1),
                Err(observed) => active = observed,
            }
        }
    }

    fn track_reserved_cpu_permit(&self, permit: OwnedSemaphorePermit) -> ChunkPipelinePermit {
        ChunkPipelinePermit {
            _permit: permit,
            active: Arc::clone(&self.metrics.active_cpu),
            idle_changed: Arc::clone(&self.metrics.idle_changed),
            admission_changed: Some(Arc::clone(&self.cpu_admission_changed)),
        }
    }
}

/// Derive bounded worker capacity from the process-visible CPU limit.
#[must_use]
pub fn automatic_worker_limits(available_parallelism: usize) -> (usize, usize) {
    let available = available_parallelism.max(1);
    let io_workers = available.div_ceil(4);
    let cpu_workers = available.div_ceil(2);
    (io_workers, cpu_workers)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkPipelineGeneration(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkPriority {
    pub ring: u32,
    pub sequence: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkRequest {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub priority: ChunkPriority,
    pub generation: ChunkPipelineGeneration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkLoadSource {
    Region,
    Generated,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedChunk {
    pub request: ChunkRequest,
    pub source: ChunkLoadSource,
    pub payload_bytes: usize,
    pub framed_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkPipelineStopReason {
    BatchLimit,
    TimeBudget,
    SendBudget,
    LoadBudget,
    GenerateBudget,
    MemoryPressure,
    QueueFull,
    QueueEmpty,
    Complete,
}

#[derive(Debug, Default, Clone)]
pub struct ChunkScheduler {
    generation: u64,
    desired: HashSet<(i32, i32)>,
    queue: VecDeque<ChunkRequest>,
    in_flight: HashMap<(i32, i32), ChunkPipelineGeneration>,
    finished: HashSet<(i32, i32)>,
}

impl ChunkScheduler {
    #[must_use]
    pub fn new<I>(desired: I) -> Self
    where
        I: IntoIterator<Item = (i32, i32, ChunkPriority)>,
    {
        let mut scheduler = Self::default();
        scheduler.replace_view(desired);
        scheduler
    }

    pub fn replace_view<I>(&mut self, desired: I)
    where
        I: IntoIterator<Item = (i32, i32, ChunkPriority)>,
    {
        self.generation = self.generation.wrapping_add(1).max(1);
        self.desired.clear();
        self.queue.clear();
        self.in_flight.clear();

        let generation = ChunkPipelineGeneration(self.generation);
        for (chunk_x, chunk_z, priority) in desired {
            if !self.desired.insert((chunk_x, chunk_z)) {
                continue;
            }
            if self.finished.contains(&(chunk_x, chunk_z)) {
                continue;
            }
            self.queue.push_back(ChunkRequest {
                chunk_x,
                chunk_z,
                priority,
                generation,
            });
        }

        self.finished.retain(|coord| self.desired.contains(coord));
    }

    pub fn replay_view<I>(&mut self, desired: I)
    where
        I: IntoIterator<Item = (i32, i32, ChunkPriority)>,
    {
        self.finished.clear();
        self.replace_view(desired);
    }

    pub fn reprioritize_queued<I>(&mut self, desired: I)
    where
        I: IntoIterator<Item = (i32, i32, ChunkPriority)>,
    {
        let priorities: HashMap<_, _> = desired
            .into_iter()
            .enumerate()
            .map(|(rank, (chunk_x, chunk_z, priority))| ((chunk_x, chunk_z), (rank, priority)))
            .collect();
        let mut queued: Vec<_> = self.queue.drain(..).enumerate().collect();

        for (_, request) in &mut queued {
            if let Some((_, priority)) = priorities.get(&(request.chunk_x, request.chunk_z)) {
                request.priority = *priority;
            }
        }
        queued.sort_by_key(|(old_rank, request)| {
            (
                priorities
                    .get(&(request.chunk_x, request.chunk_z))
                    .map_or(usize::MAX, |(rank, _)| *rank),
                *old_rank,
            )
        });
        self.queue
            .extend(queued.into_iter().map(|(_, request)| request));
    }

    #[must_use]
    pub fn current_generation(&self) -> ChunkPipelineGeneration {
        ChunkPipelineGeneration(self.generation)
    }

    pub fn poll_next(&mut self) -> Option<ChunkRequest> {
        while let Some(request) = self.queue.pop_front() {
            let coord = (request.chunk_x, request.chunk_z);
            if !self.is_current(request) || self.finished.contains(&coord) {
                continue;
            }
            self.in_flight.insert(coord, request.generation);
            return Some(request);
        }
        None
    }

    pub fn mark_finished(&mut self, request: ChunkRequest) -> bool {
        if !self.is_current(request) {
            return false;
        }

        let coord = (request.chunk_x, request.chunk_z);
        let Some(generation) = self.in_flight.get(&coord).copied() else {
            return false;
        };
        if generation != request.generation {
            return false;
        }

        self.in_flight.remove(&coord);
        self.finished.insert(coord);
        true
    }

    pub fn defer(&mut self, request: ChunkRequest) -> bool {
        if !self.is_current(request) {
            return false;
        }

        let coord = (request.chunk_x, request.chunk_z);
        let Some(generation) = self.in_flight.get(&coord).copied() else {
            return false;
        };
        if generation != request.generation {
            return false;
        }

        self.in_flight.remove(&coord);
        self.queue.push_back(request);
        true
    }

    pub fn defer_front(&mut self, request: ChunkRequest) -> bool {
        if !self.is_current(request) {
            return false;
        }

        let coord = (request.chunk_x, request.chunk_z);
        let Some(generation) = self.in_flight.get(&coord).copied() else {
            return false;
        };
        if generation != request.generation {
            return false;
        }

        self.in_flight.remove(&coord);
        self.queue.push_front(request);
        true
    }

    #[must_use]
    pub fn is_current(&self, request: ChunkRequest) -> bool {
        request.generation == ChunkPipelineGeneration(self.generation)
            && self.desired.contains(&(request.chunk_x, request.chunk_z))
    }

    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.finished.len() == self.desired.len()
            && self.queue.is_empty()
            && self.in_flight.is_empty()
    }

    #[must_use]
    pub fn desired_len(&self) -> usize {
        self.desired.len()
    }

    #[must_use]
    pub fn queued_len(&self) -> usize {
        self.queue.len()
    }

    #[must_use]
    pub fn in_flight_len(&self) -> usize {
        self.in_flight.len()
    }

    #[must_use]
    pub fn finished_len(&self) -> usize {
        self.finished.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn priority(sequence: u32) -> ChunkPriority {
        ChunkPriority { ring: 0, sequence }
    }

    #[test]
    fn resource_metrics_record_stop_reasons_without_touching_active_permits() {
        let resources = ChunkPipelineResources::with_limits(1, 1);
        let metrics = resources.metrics();

        resources.record_stop_reason(ChunkPipelineStopReason::QueueFull);
        resources.record_stop_reason(ChunkPipelineStopReason::QueueFull);
        resources.record_stop_reason(ChunkPipelineStopReason::SendBudget);

        let active = metrics.snapshot();
        assert_eq!(active.active_io, 0);
        assert_eq!(active.active_cpu, 0);
        let counts = metrics.stop_reason_counts();
        assert_eq!(counts.queue_full, 2);
        assert_eq!(counts.send_budget, 1);
        assert_eq!(
            metrics.observed_stop_reasons(),
            vec![
                ChunkPipelineStopReason::SendBudget,
                ChunkPipelineStopReason::QueueFull,
            ]
        );
    }

    #[test]
    fn resource_metrics_keep_the_maximum_observed_result_queue_depth() {
        let resources = ChunkPipelineResources::with_limits(1, 1);
        let metrics = resources.metrics();

        resources.observe_result_queue_depth(2);
        resources.observe_result_queue_depth(7);
        resources.observe_result_queue_depth(3);

        assert_eq!(metrics.snapshot().max_result_queue_depth, 7);
    }

    #[tokio::test]
    async fn wait_for_idle_wakes_when_last_active_permit_drops() {
        let resources = ChunkPipelineResources::with_limits(1, 1);
        let io_permit = resources.acquire_io().await.unwrap();
        let cpu_permit = resources.acquire_cpu().await.unwrap();
        let mut idle = std::pin::pin!(resources.wait_for_idle());

        let (probe_tx, probe_rx) = tokio::sync::oneshot::channel();
        probe_tx.send(()).unwrap();
        tokio::select! {
            biased;
            () = &mut idle => panic!("pipeline reported idle with active permits"),
            result = probe_rx => result.unwrap(),
        }

        drop(io_permit);
        let (probe_tx, probe_rx) = tokio::sync::oneshot::channel();
        probe_tx.send(()).unwrap();
        tokio::select! {
            biased;
            () = &mut idle => panic!("pipeline reported idle with active CPU permit"),
            result = probe_rx => result.unwrap(),
        }

        drop(cpu_permit);
        tokio::time::timeout(std::time::Duration::from_secs(1), idle)
            .await
            .expect("last permit drop must wake idle waiter");
    }

    #[tokio::test]
    async fn wait_for_idle_includes_async_prepare_task_lifetime() {
        let resources = ChunkPipelineResources::with_limits(1, 1);
        let task = resources.begin_prepare_task();
        let permit = resources.acquire_cpu().await.unwrap();
        drop(permit);
        let mut idle = std::pin::pin!(resources.wait_for_idle());

        let (probe_tx, probe_rx) = tokio::sync::oneshot::channel();
        probe_tx.send(()).unwrap();
        tokio::select! {
            biased;
            () = &mut idle => panic!("pipeline reported idle before prepare task completed"),
            result = probe_rx => result.unwrap(),
        }

        drop(task);
        tokio::time::timeout(std::time::Duration::from_secs(1), idle)
            .await
            .expect("prepare task completion must wake idle waiter");
    }

    #[tokio::test]
    async fn adaptive_cpu_limit_wakes_waiter_on_scale_up() {
        let resources = ChunkPipelineResources::with_limits(1, 4);
        let first = resources.acquire_cpu().await.unwrap();
        let second = resources.acquire_cpu().await.unwrap();

        assert_eq!(
            resources.apply_runtime_control_action(crate::AutoscaleAction::ScaleDown, false),
            2
        );
        assert_eq!(resources.cpu_limit(), 2);
        assert!(resources.try_acquire_cpu().is_none());

        let mut third = std::pin::pin!(resources.acquire_cpu());
        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(third.as_mut(), cx).is_pending(),
                "reduced CPU admission must hold new background work"
            );
            std::task::Poll::Ready(())
        })
        .await;

        assert_eq!(
            resources.apply_runtime_control_action(crate::AutoscaleAction::ScaleUp, false),
            4
        );
        let third = third.await.unwrap();
        assert_eq!(resources.metrics().snapshot().active_cpu, 3);

        drop((first, second, third));
    }

    #[tokio::test]
    async fn adaptive_cpu_limit_wakes_waiter_after_active_work_releases() {
        let resources = ChunkPipelineResources::with_limits(1, 2);
        let first = resources.acquire_cpu().await.unwrap();
        let second = resources.acquire_cpu().await.unwrap();
        assert_eq!(
            resources.apply_runtime_control_action(crate::AutoscaleAction::ScaleDown, false),
            1
        );

        let mut waiting = std::pin::pin!(resources.acquire_cpu());
        std::future::poll_fn(|cx| {
            assert!(std::future::Future::poll(waiting.as_mut(), cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;

        drop(first);
        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(waiting.as_mut(), cx).is_pending(),
                "active work must fall below the new limit before admission"
            );
            std::task::Poll::Ready(())
        })
        .await;

        drop(second);
        let waiting = waiting.await.unwrap();
        assert_eq!(resources.metrics().snapshot().active_cpu, 1);
        drop(waiting);
    }

    #[tokio::test]
    async fn prepare_request_admission_shares_the_runtime_cpu_limit() {
        let resources = ChunkPipelineResources::with_limits(1, 2);
        assert_eq!(
            resources.apply_runtime_control_action(crate::AutoscaleAction::ScaleDown, false),
            1
        );
        let first = resources.acquire_prepare_request().await.unwrap();
        assert!(resources.try_acquire_prepare_request().is_none());

        let mut waiting = std::pin::pin!(resources.acquire_prepare_request());
        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(waiting.as_mut(), cx).is_pending(),
                "a second batch must not bypass global request admission"
            );
            std::task::Poll::Ready(())
        })
        .await;

        drop(first);
        let second = waiting.await.unwrap();
        drop(second);
    }

    #[test]
    fn automatic_worker_limits_reserve_runtime_cpu_and_scale_io() {
        assert_eq!(automatic_worker_limits(1), (1, 1));
        assert_eq!(automatic_worker_limits(4), (1, 2));
        assert_eq!(automatic_worker_limits(8), (2, 4));
        assert_eq!(automatic_worker_limits(32), (8, 16));
    }

    #[test]
    fn scheduler_dedupes_desired_chunks() {
        let scheduler = ChunkScheduler::new([
            (0, 0, priority(0)),
            (0, 0, priority(1)),
            (1, 0, priority(2)),
        ]);

        assert_eq!(scheduler.desired_len(), 2);
        assert_eq!(scheduler.queued_len(), 2);
    }

    #[test]
    fn scheduler_replay_view_requeues_finished_chunks() {
        let mut scheduler = ChunkScheduler::new([(0, 0, priority(0)), (1, 0, priority(1))]);
        let first = scheduler.poll_next().expect("first request");
        assert!(scheduler.mark_finished(first));
        let second = scheduler.poll_next().expect("second request");
        assert!(scheduler.mark_finished(second));
        assert!(scheduler.is_complete());

        scheduler.replay_view([(0, 0, priority(0)), (1, 0, priority(1))]);

        assert_eq!(scheduler.queued_len(), 2);
        assert_eq!(scheduler.finished_len(), 0);
    }

    #[test]
    fn scheduler_reprioritizes_only_queued_requests() {
        let mut scheduler = ChunkScheduler::new([
            (0, 0, priority(0)),
            (1, 0, priority(1)),
            (2, 0, priority(2)),
            (3, 0, priority(3)),
        ]);
        let finished = scheduler.poll_next().expect("finished request");
        assert!(scheduler.mark_finished(finished));
        let in_flight = scheduler.poll_next().expect("in-flight request");
        let generation = scheduler.current_generation();

        scheduler.reprioritize_queued([
            (3, 0, priority(0)),
            (2, 0, priority(1)),
            (1, 0, priority(2)),
            (0, 0, priority(3)),
        ]);

        assert_eq!(scheduler.current_generation(), generation);
        assert_eq!(scheduler.finished_len(), 1);
        assert_eq!(scheduler.in_flight_len(), 1);
        assert!(scheduler.mark_finished(in_flight));
        let first_reprioritized = scheduler.poll_next().expect("reprioritized request");
        assert_eq!(
            (first_reprioritized.chunk_x, first_reprioritized.chunk_z),
            (3, 0)
        );
        assert_eq!(first_reprioritized.priority, priority(0));
    }

    #[test]
    fn scheduler_tracks_in_flight_and_completion() {
        let mut scheduler = ChunkScheduler::new([(0, 0, priority(0)), (1, 0, priority(1))]);

        let first = scheduler.poll_next().expect("first request");
        assert_eq!(scheduler.in_flight_len(), 1);
        assert!(!scheduler.is_complete());

        assert!(scheduler.mark_finished(first));
        assert_eq!(scheduler.finished_len(), 1);
        assert_eq!(scheduler.in_flight_len(), 0);

        let second = scheduler.poll_next().expect("second request");
        assert!(scheduler.mark_finished(second));
        assert!(scheduler.is_complete());
    }

    #[test]
    fn scheduler_rejects_stale_generation_results() {
        let mut scheduler = ChunkScheduler::new([(0, 0, priority(0))]);
        let stale = scheduler.poll_next().expect("old request");

        scheduler.replace_view([(1, 0, priority(0))]);

        assert!(!scheduler.mark_finished(stale));
        assert_eq!(scheduler.finished_len(), 0);

        let current = scheduler.poll_next().expect("current request");
        assert!(scheduler.mark_finished(current));
        assert!(scheduler.is_complete());
    }

    #[test]
    fn scheduler_requeues_still_desired_in_flight_chunks_after_replan() {
        let mut scheduler = ChunkScheduler::new([(0, 0, priority(0))]);
        let stale = scheduler.poll_next().expect("old request");

        scheduler.replace_view([(0, 0, priority(0))]);

        assert!(!scheduler.mark_finished(stale));
        let current = scheduler.poll_next().expect("requeued request");
        assert_ne!(current.generation, stale.generation);
        assert_eq!((current.chunk_x, current.chunk_z), (0, 0));
        assert!(scheduler.mark_finished(current));
        assert!(scheduler.is_complete());
    }

    #[test]
    fn scheduler_defer_requeues_without_finishing() {
        let mut scheduler = ChunkScheduler::new([(0, 0, priority(0))]);

        let request = scheduler.poll_next().expect("request");
        assert!(scheduler.defer(request));

        assert_eq!(scheduler.finished_len(), 0);
        assert_eq!(scheduler.in_flight_len(), 0);
        assert_eq!(scheduler.queued_len(), 1);
        let retried = scheduler.poll_next().expect("retried request");
        assert_eq!((retried.chunk_x, retried.chunk_z), (0, 0));
        assert_eq!(retried.generation, request.generation);
    }

    #[test]
    fn scheduler_defer_rejects_generation_mismatch_without_dropping_in_flight() {
        let mut scheduler = ChunkScheduler::new([(0, 0, priority(0))]);
        let mut request = scheduler.poll_next().expect("request");
        request.generation = ChunkPipelineGeneration(request.generation.0 + 1);

        assert!(!scheduler.defer(request));
        assert_eq!(scheduler.in_flight_len(), 1);
        assert_eq!(scheduler.queued_len(), 0);
    }

    #[test]
    fn resources_share_global_permit_budget_across_clones() {
        let resources = ChunkPipelineResources::with_limits(1, 1);
        let other_stream_resources = resources.clone();

        let io_permit = Arc::clone(&resources.io_permits)
            .try_acquire_owned()
            .expect("first IO permit");
        assert!(
            Arc::clone(&other_stream_resources.io_permits)
                .try_acquire_owned()
                .is_err()
        );
        drop(io_permit);
        assert!(
            Arc::clone(&other_stream_resources.io_permits)
                .try_acquire_owned()
                .is_ok()
        );

        let cpu_permit = Arc::clone(&resources.cpu_permits)
            .try_acquire_owned()
            .expect("first CPU permit");
        assert!(
            Arc::clone(&other_stream_resources.cpu_permits)
                .try_acquire_owned()
                .is_err()
        );
        drop(cpu_permit);
        assert!(
            Arc::clone(&other_stream_resources.cpu_permits)
                .try_acquire_owned()
                .is_ok()
        );
    }
}
