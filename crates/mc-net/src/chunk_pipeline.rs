//! Chunk-pipeline policy and hand-off types.
//!
//! M13 starts by naming the scheduler/worker boundary before moving work
//! across it. The policy is runtime configuration; the request/result
//! types describe the ownership we want between Play socket tasks and the
//! bounded chunk workers.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::{AcquireError, OwnedSemaphorePermit, Semaphore};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkPipelinePolicy {
    pub chunk_send_rate: u32,
    pub chunk_load_rate: u32,
    pub chunk_generate_rate: u32,
    pub chunk_prepare_budget_ms: u64,
    pub chunk_prepare_batch_size: usize,
    pub chunk_io_threads: usize,
    pub chunk_worker_threads: usize,
    pub entity_worker_threads: usize,
    pub chunk_result_queue_size: usize,
    pub region_cache_size: usize,
    pub compression_threshold: i32,
    pub compression_level: Option<u32>,
}

impl Default for ChunkPipelinePolicy {
    fn default() -> Self {
        Self {
            chunk_send_rate: 16,
            chunk_load_rate: 64,
            chunk_generate_rate: 32,
            chunk_prepare_budget_ms: 0,
            chunk_prepare_batch_size: 8,
            chunk_io_threads: 2,
            chunk_worker_threads: default_worker_threads(),
            entity_worker_threads: 2,
            chunk_result_queue_size: 64,
            region_cache_size: 4,
            compression_threshold: crate::login::LOGIN_COMPRESSION_THRESHOLD,
            compression_level: None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ChunkPipelineResources {
    io_permits: Arc<Semaphore>,
    cpu_permits: Arc<Semaphore>,
    metrics: ChunkPipelineResourceMetrics,
}

#[derive(Debug, Clone, Default)]
pub struct ChunkPipelineResourceMetrics {
    active_io: Arc<AtomicUsize>,
    max_io_active: Arc<AtomicUsize>,
    active_cpu: Arc<AtomicUsize>,
    max_cpu_active: Arc<AtomicUsize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkPipelineResourceSnapshot {
    pub active_io: usize,
    pub max_io_active: usize,
    pub active_cpu: usize,
    pub max_cpu_active: usize,
}

pub(crate) struct ChunkPipelinePermit {
    _permit: OwnedSemaphorePermit,
    active: Arc<AtomicUsize>,
}

impl Drop for ChunkPipelinePermit {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
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
        }
    }
}

impl ChunkPipelineResources {
    #[must_use]
    pub(crate) fn new(policy: ChunkPipelinePolicy) -> Self {
        Self::with_limits(policy.chunk_io_threads, policy.chunk_worker_threads)
    }

    #[must_use]
    pub(crate) fn with_limits(chunk_io_threads: usize, chunk_worker_threads: usize) -> Self {
        Self {
            io_permits: Arc::new(Semaphore::new(chunk_io_threads.max(1))),
            cpu_permits: Arc::new(Semaphore::new(chunk_worker_threads.max(1))),
            metrics: ChunkPipelineResourceMetrics::default(),
        }
    }

    #[must_use]
    pub(crate) fn metrics(&self) -> ChunkPipelineResourceMetrics {
        self.metrics.clone()
    }

    pub(crate) async fn acquire_io(&self) -> Result<ChunkPipelinePermit, AcquireError> {
        let permit = Arc::clone(&self.io_permits).acquire_owned().await?;
        Ok(self.track_permit(permit, true))
    }

    pub(crate) async fn acquire_cpu(&self) -> Result<ChunkPipelinePermit, AcquireError> {
        let permit = Arc::clone(&self.cpu_permits).acquire_owned().await?;
        Ok(self.track_permit(permit, false))
    }

    fn track_permit(&self, permit: OwnedSemaphorePermit, io: bool) -> ChunkPipelinePermit {
        let (active, max_active) = if io {
            (&self.metrics.active_io, &self.metrics.max_io_active)
        } else {
            (&self.metrics.active_cpu, &self.metrics.max_cpu_active)
        };
        let now = active.fetch_add(1, Ordering::AcqRel) + 1;
        max_active.fetch_max(now, Ordering::AcqRel);
        ChunkPipelinePermit {
            _permit: permit,
            active: Arc::clone(active),
        }
    }
}

fn default_worker_threads() -> usize {
    4
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
