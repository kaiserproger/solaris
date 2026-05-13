//! Chunk-pipeline policy and hand-off types.
//!
//! M13 starts by naming the scheduler/worker boundary before moving work
//! across it. The policy is runtime configuration; the request/result
//! types describe the ownership we want between Play socket tasks and the
//! bounded chunk workers.

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
