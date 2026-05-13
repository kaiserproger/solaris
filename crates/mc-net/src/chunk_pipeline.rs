//! Chunk-pipeline policy and hand-off types.
//!
//! M13 starts by naming the scheduler/worker boundary before moving work
//! across it. The policy is runtime configuration; the request/result
//! types describe the ownership we want between Play socket tasks and the
//! bounded chunk workers.

use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkPipelinePolicy {
    pub chunk_send_rate: u32,
    pub chunk_load_rate: u32,
    pub chunk_generate_rate: u32,
    pub chunk_prepare_budget_ms: u64,
    pub chunk_prepare_batch_size: usize,
    pub chunk_io_threads: usize,
    pub chunk_worker_threads: usize,
    pub chunk_result_queue_size: usize,
    pub region_cache_size: usize,
}

impl Default for ChunkPipelinePolicy {
    fn default() -> Self {
        Self {
            chunk_send_rate: 64,
            chunk_load_rate: 64,
            chunk_generate_rate: 32,
            chunk_prepare_budget_ms: 0,
            chunk_prepare_batch_size: 1,
            chunk_io_threads: 2,
            chunk_worker_threads: default_worker_threads(),
            chunk_result_queue_size: 64,
            region_cache_size: 4,
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
        let Some(generation) = self.in_flight.remove(&coord) else {
            return false;
        };
        if generation != request.generation {
            return false;
        }

        self.finished.insert(coord);
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
}
