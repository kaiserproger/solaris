//! Low-overhead CPU counters; thread clocks measure execution, never await time.
use std::cell::Cell;
use std::future::Future;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum CpuStage {
    ChunkDisk,
    ChunkGeneration,
    ChunkData,
    Heightmaps,
    Lighting,
    LightEncoding,
    PacketFraming,
    Simulation,
    Saving,
    Network,
    ChunkPreparation,
}
const NAMES: [&str; 11] = [
    "chunk_disk_decode",
    "chunk_generation",
    "chunk_block_encoding",
    "heightmaps",
    "lighting",
    "light_encoding",
    "packet_framing_compression",
    "simulation",
    "saving",
    "network",
    "chunk_preparation_other",
];
struct Counters {
    calls: AtomicU64,
    cpu_ns: AtomicU64,
    wall_ns: AtomicU64,
    max_cpu_ns: AtomicU64,
}
static COUNTERS: [Counters; 11] = [const {
    Counters {
        calls: AtomicU64::new(0),
        cpu_ns: AtomicU64::new(0),
        wall_ns: AtomicU64::new(0),
        max_cpu_ns: AtomicU64::new(0),
    }
}; 11];
thread_local! { static COMPLETED_CPU: Cell<u64> = const { Cell::new(0) }; }

#[cfg(target_os = "linux")]
fn clock_ns(clock: rustix::time::ClockId) -> u64 {
    let value = rustix::time::clock_gettime(clock);
    (value.tv_sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(value.tv_nsec as u64)
}
/// Process CPU includes all threads and native dependencies.
pub fn process_cpu_ns() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        Some(clock_ns(rustix::time::ClockId::ProcessCPUTime))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}
fn thread_cpu_ns() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        Some(clock_ns(rustix::time::ClockId::ThreadCPUTime))
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// !Send deliberately prevents carrying a thread-clock scope across an await.
pub(crate) struct CpuScope {
    stage: CpuStage,
    cpu: Option<u64>,
    children: u64,
    wall: Instant,
    _thread: PhantomData<Rc<()>>,
}
impl CpuScope {
    pub(crate) fn new(stage: CpuStage) -> Self {
        Self {
            stage,
            cpu: thread_cpu_ns(),
            children: COMPLETED_CPU.get(),
            wall: Instant::now(),
            _thread: PhantomData,
        }
    }
}
impl Drop for CpuScope {
    fn drop(&mut self) {
        let counters = &COUNTERS[self.stage as usize];
        if let (Some(start), Some(end)) = (self.cpu, thread_cpu_ns()) {
            let children = COMPLETED_CPU.get().wrapping_sub(self.children);
            let exclusive = end.saturating_sub(start).saturating_sub(children);
            COMPLETED_CPU.set(COMPLETED_CPU.get().wrapping_add(exclusive));
            counters.cpu_ns.fetch_add(exclusive, Ordering::Relaxed);
            counters.max_cpu_ns.fetch_max(exclusive, Ordering::Relaxed);
        }
        counters.calls.fetch_add(1, Ordering::Relaxed);
        counters.wall_ns.fetch_add(
            self.wall.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64,
            Ordering::Relaxed,
        );
    }
}

/// Scope each poll independently: migration and suspended time cannot be
/// attributed to the task's CPU. Nested synchronous stages are exclusive.
pub(crate) async fn measure_future<T>(stage: CpuStage, future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    std::future::poll_fn(|cx| {
        let _scope = CpuScope::new(stage);
        future.as_mut().poll(cx)
    })
    .await
}

pub(crate) fn measure<T>(stage: CpuStage, work: impl FnOnce() -> T) -> T {
    let _scope = CpuScope::new(stage);
    work()
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CpuStageSnapshot {
    pub name: &'static str,
    pub executions_or_polls: u64,
    pub exclusive_cpu_ns: Option<u64>,
    pub inclusive_execution_wall_ns: u64,
    pub max_execution_cpu_ns: Option<u64>,
}
pub fn cpu_stages() -> Vec<CpuStageSnapshot> {
    NAMES
        .iter()
        .zip(&COUNTERS)
        .map(|(&name, c)| CpuStageSnapshot {
            name,
            executions_or_polls: c.calls.load(Ordering::Relaxed),
            exclusive_cpu_ns: cfg!(target_os = "linux").then(|| c.cpu_ns.load(Ordering::Relaxed)),
            inclusive_execution_wall_ns: c.wall_ns.load(Ordering::Relaxed),
            max_execution_cpu_ns: cfg!(target_os = "linux")
                .then(|| c.max_cpu_ns.load(Ordering::Relaxed)),
        })
        .collect()
}

static LIGHT_WORKSPACE_BYTES: AtomicUsize = AtomicUsize::new(0);
static LIGHT_WORKSPACE_PEAK: AtomicUsize = AtomicUsize::new(0);
static LIGHT_WORKSPACE_COUNT: AtomicUsize = AtomicUsize::new(0);
#[derive(Default)]
pub(crate) struct LightWorkspaceGauge {
    bytes: usize,
    registered: bool,
}
impl LightWorkspaceGauge {
    pub(crate) fn update(&mut self, bytes: usize) {
        if !self.registered {
            LIGHT_WORKSPACE_COUNT.fetch_add(1, Ordering::Relaxed);
            self.registered = true;
        }
        let total = if bytes >= self.bytes {
            LIGHT_WORKSPACE_BYTES.fetch_add(bytes - self.bytes, Ordering::Relaxed) + bytes
                - self.bytes
        } else {
            LIGHT_WORKSPACE_BYTES.fetch_sub(self.bytes - bytes, Ordering::Relaxed)
                - (self.bytes - bytes)
        };
        self.bytes = bytes;
        LIGHT_WORKSPACE_PEAK.fetch_max(total, Ordering::Relaxed);
    }
}
impl Drop for LightWorkspaceGauge {
    fn drop(&mut self) {
        LIGHT_WORKSPACE_BYTES.fetch_sub(self.bytes, Ordering::Relaxed);
        if self.registered {
            LIGHT_WORKSPACE_COUNT.fetch_sub(1, Ordering::Relaxed);
        }
    }
}
pub fn light_workspaces() -> serde_json::Value {
    serde_json::json!({"allocated_capacity_bytes": LIGHT_WORKSPACE_BYTES.load(Ordering::Relaxed), "peak_capacity_bytes": LIGHT_WORKSPACE_PEAK.load(Ordering::Relaxed), "worker_count": LIGHT_WORKSPACE_COUNT.load(Ordering::Relaxed), "sampling": "updated after each lighting build; in-flight growth appears at completion"})
}
