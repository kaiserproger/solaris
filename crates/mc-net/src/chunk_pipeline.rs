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

#[cfg(test)]
mod resource_admission_tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkPipelinePolicy {
    /// Per-tick dispatch cap for chunk sends. `u32::MAX` means uncapped: the
    /// worker pool is the throttle, and runtime control may clamp the rate.
    pub chunk_send_rate: u32,
    /// Per-tick dispatch cap for chunk loads. `u32::MAX` means uncapped.
    pub chunk_load_rate: u32,
    /// Per-tick dispatch cap for chunk generation. `u32::MAX` means uncapped.
    pub chunk_generate_rate: u32,
    pub chunk_prepare_budget_ms: u64,
    pub chunk_prepare_batch_size: usize,
    /// Growth ceiling for the chunk IO worker pool. Pools start at one
    /// worker; this bounds how far sustained backlog may grow the pool.
    pub chunk_io_threads: usize,
    /// Growth ceiling for the shared chunk/physics CPU worker pool. Pools
    /// start at one worker; this bounds adaptive growth, it reserves nothing.
    pub chunk_worker_threads: usize,
    pub chunk_result_queue_size: usize,
    pub region_cache_size: usize,
    pub compression_threshold: i32,
    pub compression_level: Option<u32>,
    pub runtime_control: Option<RuntimeControlConfig>,
}

impl Default for ChunkPipelinePolicy {
    /// The adaptive default. Rates are uncapped (`u32::MAX`) and both
    /// physical pools start with a single worker; the thread fields are the
    /// growth ceilings the runtime control plane may expand into under
    /// sustained backlog. Nothing is reserved up front. Operators override
    /// any field explicitly and overrides are honored verbatim.
    fn default() -> Self {
        let available = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1);
        Self {
            chunk_send_rate: u32::MAX,
            chunk_load_rate: u32::MAX,
            chunk_generate_rate: u32::MAX,
            chunk_prepare_budget_ms: 0,
            chunk_prepare_batch_size: 8,
            chunk_io_threads: available,
            chunk_worker_threads: available,
            chunk_result_queue_size: 64,
            region_cache_size: 4,
            compression_threshold: crate::login::LOGIN_COMPRESSION_THRESHOLD,
            compression_level: None,
            runtime_control: None,
        }
    }
}

impl ChunkPipelinePolicy {
    /// A policy whose pools are capped to `workers` shared CPU workers and one
    /// IO worker, with every other knob at its derived default.
    ///
    /// Used by in-process servers that already share a machine with other
    /// workgroups - a test binary that starts several servers, or a host that
    /// bounded the process on purpose. Like `Default`, the pools start at one
    /// worker each; the bound is the adaptive growth ceiling, never a
    /// reservation.
    #[must_use]
    pub fn bounded(workers: usize) -> Self {
        Self {
            chunk_io_threads: 1,
            chunk_worker_threads: workers.max(1),
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ChunkPipelineResources {
    io_permits: Arc<Semaphore>,
    cpu_permits: Arc<Semaphore>,
    prepare_request_permits: Arc<Semaphore>,
    /// Live IO workers in existence; the pool grows toward `io_capacity`.
    io_live: Arc<AtomicUsize>,
    /// Live CPU workers in existence; the pool grows toward `cpu_capacity`.
    cpu_live: Arc<AtomicUsize>,
    /// Consecutive fully-idle runtime-control decisions; drives pool decay.
    idle_decision_streak: Arc<AtomicUsize>,
    /// Adaptive growth ceilings from policy. They never reserve workers.
    io_capacity: usize,
    cpu_capacity: usize,
    prepare_limit: Arc<AtomicUsize>,
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
    changed: Notify,
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

    pub async fn wait_for_stream_cancellation_after(
        &self,
        previous_cancelled_streams: usize,
    ) -> ChunkPipelineCancellationSnapshot {
        loop {
            let changed = self.cancellations.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let snapshot = self.cancellation_snapshot();
            if snapshot.cancelled_streams > previous_cancelled_streams {
                return snapshot;
            }
            changed.await;
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
            .cancelled_requests
            .fetch_add(requests, Ordering::Relaxed);
        self.cancellations
            .cancelled_streams
            .fetch_add(1, Ordering::Release);
        self.cancellations.changed.notify_waiters();
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
/// Sustained full-idle runtime-control decisions after which an adaptive
/// pool returns one step - roughly a minute of the ~50 ms control tick with
/// no worker pickup in between.
const IDLE_POOL_SHRINK_AFTER_DECISIONS: usize = 1_200;

impl ChunkPipelineResources {
    /// Adaptive pool: starts with a single IO and a single CPU worker; the
    /// policy thread fields only bound how far runtime-control decisions may
    /// grow the pools under sustained backlog.
    #[must_use]
    pub(crate) fn new(policy: ChunkPipelinePolicy) -> Self {
        let mut resources = Self::with_limits(1, 1);
        resources.io_capacity = policy.chunk_io_threads.max(1);
        resources.cpu_capacity = policy.chunk_worker_threads.max(1);
        resources
    }

    /// Fixed pool sized at construction. Used by tests and callers that pin
    /// their pools deliberately; production pools start through [`Self::new`].
    #[must_use]
    pub(crate) fn with_limits(chunk_io_threads: usize, cpu_worker_threads: usize) -> Self {
        let cpu_capacity = cpu_worker_threads.max(1);
        Self {
            io_permits: Arc::new(Semaphore::new(chunk_io_threads.max(1))),
            cpu_permits: Arc::new(Semaphore::new(cpu_capacity)),
            prepare_request_permits: Arc::new(Semaphore::new(cpu_capacity)),
            idle_decision_streak: Arc::new(AtomicUsize::new(0)),
            io_live: Arc::new(AtomicUsize::new(chunk_io_threads.max(1))),
            cpu_live: Arc::new(AtomicUsize::new(cpu_capacity)),
            io_capacity: chunk_io_threads.max(1),
            cpu_capacity,
            prepare_limit: Arc::new(AtomicUsize::new(cpu_capacity.saturating_sub(1).max(1))),
            prepare_admission_changed: Arc::new(Notify::new()),
            active_prepare_tasks: Arc::new(AtomicUsize::new(0)),
            active_prepare_requests: Arc::new(AtomicUsize::new(0)),
            metrics: ChunkPipelineResourceMetrics::default(),
        }
    }

    /// Live CPU workers in existence right now. Starts at one and follows
    /// adaptive growth, so consumers (entity lanes, regional physics) scale
    /// with the pool instead of a compile-time formula.
    #[must_use]
    pub(crate) fn cpu_capacity(&self) -> usize {
        self.cpu_live.load(Ordering::Acquire)
    }

    #[must_use]
    pub(crate) fn prepare_limit(&self) -> usize {
        self.prepare_limit.load(Ordering::Acquire)
    }

    /// Apply one runtime-control decision step to the physical pools.
    ///
    /// Sustained chunk backlog (`ScaleDown` carrying `ChunkQueue` or
    /// `FirstChunkSla`) buys one more IO and CPU worker up to the policy
    /// ceilings - one worker demonstrably could not keep up. Draining returns
    /// the pools toward the one-worker floor immediately, and a pool that has
    /// been fully idle for the sustained-idle window shrinks one step per
    /// decision. Background preparation admission keeps its existing
    /// semantics: chunk pressure throttles producers through the controller
    /// limits rather than the capacity left to drain work.
    pub(crate) fn apply_runtime_control_action(
        &self,
        action: crate::AutoscaleAction,
        pressure: Option<crate::AutoscalePressure>,
        draining: bool,
    ) -> usize {
        let grew = if draining {
            self.idle_decision_streak.store(0, Ordering::Relaxed);
            while self.shrink_worker_pools_one_step() {}
            false
        } else if action == crate::AutoscaleAction::ScaleDown
            && matches!(
                pressure,
                Some(
                    crate::AutoscalePressure::ChunkQueue | crate::AutoscalePressure::FirstChunkSla
                )
            )
        {
            self.grow_worker_pools()
        } else {
            false
        };
        if !draining && !grew {
            self.decay_idle_worker_pools();
        }
        let current = self.prepare_limit();
        let next = if draining {
            1
        } else {
            match action {
                crate::AutoscaleAction::Hold => current,
                crate::AutoscaleAction::ScaleDown
                    if pressure == Some(crate::AutoscalePressure::ChunkQueue) =>
                {
                    current
                }
                crate::AutoscaleAction::ScaleDown => current.saturating_sub(1).max(1),
                crate::AutoscaleAction::ScaleUp => current
                    .saturating_add(1)
                    .min(self.cpu_capacity().saturating_sub(1).max(1)),
            }
        };
        if self.prepare_limit.swap(next, Ordering::AcqRel) != next {
            self.prepare_admission_changed.notify_waiters();
        }
        next
    }

    /// One sustained-backlog step: add one worker per pool up to the policy
    /// ceilings. Returns true when any pool grew.
    fn grow_worker_pools(&self) -> bool {
        let mut grew = false;
        if self.cpu_live.load(Ordering::Acquire) < self.cpu_capacity {
            self.cpu_permits.add_permits(1);
            self.prepare_request_permits.add_permits(1);
            self.cpu_live.fetch_add(1, Ordering::AcqRel);
            grew = true;
        }
        if self.io_live.load(Ordering::Acquire) < self.io_capacity {
            self.io_permits.add_permits(1);
            self.io_live.fetch_add(1, Ordering::AcqRel);
            grew = true;
        }
        grew
    }

    /// One step back toward the one-worker floor. Permits only leave the pool
    /// while they are available; busy workers finish first.
    fn shrink_worker_pools_one_step(&self) -> bool {
        let mut shrank = false;
        if self.cpu_live.load(Ordering::Acquire) > 1 && self.cpu_permits.forget_permits(1) == 1 {
            self.cpu_live.fetch_sub(1, Ordering::AcqRel);
            // Best effort: surplus background admission permits stay dormant
            // because prepare_limit is clamped below the live CPU pool.
            let _ = self.prepare_request_permits.forget_permits(1);
            shrank = true;
        }
        if self.io_live.load(Ordering::Acquire) > 1 && self.io_permits.forget_permits(1) == 1 {
            self.io_live.fetch_sub(1, Ordering::AcqRel);
            shrank = true;
        }
        if shrank {
            let floor = self
                .cpu_live
                .load(Ordering::Acquire)
                .saturating_sub(1)
                .max(1);
            let clamped = self.prepare_limit().min(floor);
            if self.prepare_limit.swap(clamped, Ordering::AcqRel) != clamped {
                self.prepare_admission_changed.notify_waiters();
            }
        }
        shrank
    }

    /// Return one pool step after the pool has been fully idle for the
    /// sustained-idle decision streak: the backlog cleared, so stop holding
    /// the workers. Any busy worker or pickup resets the streak.
    fn decay_idle_worker_pools(&self) {
        if self.io_live.load(Ordering::Acquire) <= 1 && self.cpu_live.load(Ordering::Acquire) <= 1 {
            return;
        }
        let snapshot = self.metrics.snapshot();
        let fully_idle = snapshot.active_io == 0
            && snapshot.active_cpu == 0
            && self.active_prepare_tasks.load(Ordering::Acquire) == 0
            && self.active_prepare_requests.load(Ordering::Acquire) == 0;
        if !fully_idle {
            self.idle_decision_streak.store(0, Ordering::Relaxed);
            return;
        }
        let streak = self.idle_decision_streak.fetch_add(1, Ordering::Relaxed) + 1;
        if streak >= IDLE_POOL_SHRINK_AFTER_DECISIONS {
            self.idle_decision_streak.store(0, Ordering::Relaxed);
            self.shrink_worker_pools_one_step();
        }
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
        Ok(self.track_cpu_permit(permit))
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
            if self.try_reserve_prepare_request() {
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
        Some(self.track_cpu_permit(permit))
    }

    #[cfg(test)]
    pub(crate) fn try_acquire_prepare_request(&self) -> Option<ChunkPipelinePermit> {
        let permit = Arc::clone(&self.prepare_request_permits)
            .try_acquire_owned()
            .ok()?;
        if !self.try_reserve_prepare_request() {
            return None;
        }
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

    fn try_reserve_prepare_request(&self) -> bool {
        let active_slots = &self.active_prepare_requests;
        let mut active = active_slots.load(Ordering::Acquire);
        loop {
            if active >= self.prepare_limit() {
                return false;
            }
            match active_slots.compare_exchange_weak(
                active,
                active + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    return true;
                }
                Err(observed) => active = observed,
            }
        }
    }

    fn track_cpu_permit(&self, permit: OwnedSemaphorePermit) -> ChunkPipelinePermit {
        let active = &self.metrics.active_cpu;
        let now = active.fetch_add(1, Ordering::AcqRel) + 1;
        self.metrics.max_cpu_active.fetch_max(now, Ordering::AcqRel);
        ChunkPipelinePermit {
            _permit: permit,
            active: Arc::clone(active),
            idle_changed: Arc::clone(&self.metrics.idle_changed),
            admission_changed: None,
        }
    }
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
    async fn cancellation_wait_wakes_a_registered_waiter() {
        let metrics = ChunkPipelineResourceMetrics::default();
        let mut waiting = std::pin::pin!(metrics.wait_for_stream_cancellation_after(0));
        std::future::poll_fn(|cx| {
            assert!(
                std::future::Future::poll(waiting.as_mut(), cx).is_pending(),
                "wait must remain pending before a cancellation"
            );
            std::task::Poll::Ready(())
        })
        .await;

        metrics.record_stream_cancellation(3);

        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_secs(1), waiting)
                .await
                .expect("recording a stream cancellation must wake the waiter"),
            ChunkPipelineCancellationSnapshot {
                cancelled_streams: 1,
                cancelled_requests: 3,
                stale_results_rejected: 0,
            }
        );
    }

    #[tokio::test]
    async fn cancellation_wait_observes_requests_recorded_before_registration() {
        let metrics = ChunkPipelineResourceMetrics::default();
        metrics.record_stream_cancellation(4);

        assert_eq!(
            tokio::time::timeout(
                std::time::Duration::from_secs(1),
                metrics.wait_for_stream_cancellation_after(0),
            )
            .await
            .expect("an earlier stream cancellation must remain observable"),
            ChunkPipelineCancellationSnapshot {
                cancelled_streams: 1,
                cancelled_requests: 4,
                stale_results_rejected: 0,
            }
        );
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

    #[test]
    fn default_policy_starts_uncapped_with_single_worker_pools() {
        let available = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1);
        let policy = ChunkPipelinePolicy::default();
        assert_eq!(policy.chunk_send_rate, u32::MAX);
        assert_eq!(policy.chunk_load_rate, u32::MAX);
        assert_eq!(policy.chunk_generate_rate, u32::MAX);
        assert_eq!(policy.chunk_io_threads, available);
        assert_eq!(policy.chunk_worker_threads, available);

        let resources = ChunkPipelineResources::new(policy);
        assert_eq!(resources.cpu_capacity(), 1);
        assert_eq!(resources.prepare_limit(), 1);
        let first = resources.try_acquire_cpu();
        assert!(first.is_some(), "one idle CPU worker exists");
        // `first` must stay held: dropping it would release the worker permit
        // back into the pool before the second probe.
        let second = resources.try_acquire_cpu();
        assert!(second.is_none(), "no second worker is reserved up front");
        drop((first, second));
    }

    #[test]
    fn bounded_policy_caps_pools_without_reserving_them() {
        let policy = ChunkPipelinePolicy::bounded(4);
        assert_eq!(policy.chunk_io_threads, 1);
        assert_eq!(policy.chunk_worker_threads, 4);

        let resources = ChunkPipelineResources::new(policy);
        assert_eq!(resources.cpu_capacity(), 1);
        assert_eq!(resources.prepare_limit(), 1);
    }

    #[test]
    fn sustained_chunk_backlog_grows_pools_to_the_policy_ceiling() {
        let policy = ChunkPipelinePolicy {
            chunk_io_threads: 2,
            chunk_worker_threads: 3,
            ..ChunkPipelinePolicy::default()
        };
        let resources = ChunkPipelineResources::new(policy);
        for _ in 0..3 {
            resources.apply_runtime_control_action(
                crate::AutoscaleAction::ScaleDown,
                Some(crate::AutoscalePressure::ChunkQueue),
                false,
            );
        }
        assert_eq!(resources.cpu_capacity(), 3);
        assert_eq!(resources.io_live.load(Ordering::Acquire), 2);
        // Growth buys drain capacity without loosening background admission.
        assert_eq!(resources.prepare_limit(), 1);
    }

    #[test]
    fn memory_and_tick_pressure_never_grow_pools() {
        let policy = ChunkPipelinePolicy {
            chunk_io_threads: 2,
            chunk_worker_threads: 2,
            ..ChunkPipelinePolicy::default()
        };
        let resources = ChunkPipelineResources::new(policy);
        for pressure in [
            crate::AutoscalePressure::Memory,
            crate::AutoscalePressure::TickTime,
        ] {
            resources.apply_runtime_control_action(
                crate::AutoscaleAction::ScaleDown,
                Some(pressure),
                false,
            );
            assert_eq!(resources.cpu_capacity(), 1);
            assert_eq!(resources.io_live.load(Ordering::Acquire), 1);
        }
    }

    #[test]
    fn sustained_idle_shrinks_pools_back_toward_one_worker() {
        let policy = ChunkPipelinePolicy {
            chunk_io_threads: 1,
            chunk_worker_threads: 3,
            ..ChunkPipelinePolicy::default()
        };
        let resources = ChunkPipelineResources::new(policy);
        for _ in 0..2 {
            resources.apply_runtime_control_action(
                crate::AutoscaleAction::ScaleDown,
                Some(crate::AutoscalePressure::ChunkQueue),
                false,
            );
        }
        assert_eq!(resources.cpu_capacity(), 3);

        for expected in [2, 1, 1] {
            for _ in 0..=IDLE_POOL_SHRINK_AFTER_DECISIONS {
                resources.apply_runtime_control_action(crate::AutoscaleAction::Hold, None, false);
            }
            assert_eq!(resources.cpu_capacity(), expected);
        }
    }

    #[test]
    fn draining_returns_pools_to_the_one_worker_floor() {
        let policy = ChunkPipelinePolicy {
            chunk_io_threads: 2,
            chunk_worker_threads: 3,
            ..ChunkPipelinePolicy::default()
        };
        let resources = ChunkPipelineResources::new(policy);
        for _ in 0..2 {
            resources.apply_runtime_control_action(
                crate::AutoscaleAction::ScaleDown,
                Some(crate::AutoscalePressure::ChunkQueue),
                false,
            );
        }
        assert_eq!(resources.cpu_capacity(), 3);
        resources.apply_runtime_control_action(crate::AutoscaleAction::Hold, None, true);
        assert_eq!(resources.cpu_capacity(), 1);
        assert_eq!(resources.io_live.load(Ordering::Acquire), 1);
        assert_eq!(resources.prepare_limit(), 1);
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
