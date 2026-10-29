//! Batches the GPU solid fills chunk workers ask for (feature `gpu`).
//!
//! A GPU fill costs about the same for one chunk as for dozens, so a request
//! queues the whole square tile of chunks around it (chunk requests sweep
//! outward from players, so the tile's other chunks follow soon) and one
//! dispatcher thread fills the queue in batches while workers block in
//! [`GpuSolidService::solids_around`]. The batch cap, and with it the tile, starts
//! at [`FIRST_CAP`] (bounded by the adapter's buffer and dispatch limits),
//! halves when a batch overruns [`BATCH_BUDGET`] and doubles back while full
//! batches finish within half of it. A chunk whose solids would wait behind two
//! capped batches evaluates density on the CPU instead, and a GPU failure turns
//! the service off for the rest of the run.
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mc_world::ChunkPos;

use super::solids::{ChunkSolids, SolidGrid};
use crate::vanilla_density::gpu::GpuDensity;

/// GPU time one batch may take before the cap halves.
const BATCH_BUDGET: Duration = Duration::from_millis(20);
const FIRST_CAP: usize = 64;
/// Filled chunks kept for later requests (12 KiB each in the overworld).
/// A chunk leaves earlier once all nine generations that read it are done.
const CACHE_CHUNKS: usize = 4096;
/// Generations that read one chunk's solids: its own and its 8 neighbours'.
const READERS: u8 = 9;

pub(crate) struct GpuSolidService {
    shared: Arc<Shared>,
    dispatcher: Option<JoinHandle<()>>,
}

struct Shared {
    state: Mutex<State>,
    /// Signalled when work is queued or the service stops.
    work: Condvar,
    /// Signalled after every batch.
    filled: Condvar,
}

struct State {
    solids: HashMap<(i32, i32), Held>,
    /// Fill order of `solids`, oldest first, for eviction. Entries whose
    /// sequence no longer matches were released or refilled since.
    order: VecDeque<((i32, i32), u64)>,
    filled: u64,
    queued: VecDeque<(i32, i32)>,
    /// Queued or in a running batch.
    wanted: HashSet<(i32, i32)>,
    cap: usize,
    failed: bool,
    stop: bool,
}

struct Held {
    solids: Arc<ChunkSolids>,
    readers: u8,
    sequence: u64,
}

impl State {
    /// Queue every chunk of `key`'s tile that is neither held nor wanted.
    fn queue_tile(&mut self, key: (i32, i32)) {
        let side = tile_side(self.cap);
        let (x0, z0) = (key.0.div_euclid(side) * side, key.1.div_euclid(side) * side);
        for z in z0..z0 + side {
            for x in x0..x0 + side {
                if !self.solids.contains_key(&(x, z)) && self.wanted.insert((x, z)) {
                    self.queued.push_back((x, z));
                }
            }
        }
    }
}

/// Largest power-of-two square side whose tile fits in one batch.
fn tile_side(cap: usize) -> i32 {
    let mut side = 1;
    while (side * 2) * (side * 2) <= cap {
        side *= 2;
    }
    side as i32
}

impl GpuSolidService {
    pub(crate) fn spawn(gpu: GpuDensity) -> std::io::Result<Self> {
        let max = gpu.max_batch().clamp(1, CACHE_CHUNKS / 8);
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                solids: HashMap::new(),
                order: VecDeque::new(),
                filled: 0,
                queued: VecDeque::new(),
                wanted: HashSet::new(),
                cap: FIRST_CAP.min(max),
                failed: false,
                stop: false,
            }),
            work: Condvar::new(),
            filled: Condvar::new(),
        });
        let dispatcher = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("worldgen-gpu".into())
                .spawn(move || dispatch(&gpu, &shared, max))?
        };
        Ok(Self {
            shared,
            dispatcher: Some(dispatcher),
        })
    }

    /// Solids of `pos` and its eight neighbours (everything an ore halo and
    /// the tree footprint read), or `None` when this chunk should evaluate
    /// density on the CPU: the GPU failed or is two batches behind.
    pub(crate) fn solids_around(&self, pos: ChunkPos) -> Option<SolidGrid> {
        let keys: [(i32, i32); 9] =
            std::array::from_fn(|i| (pos.x + i as i32 % 3 - 1, pos.z + i as i32 / 3 - 1));
        let center = (pos.x, pos.z);
        let mut state = self.shared.lock();
        let behind = state.queued.len();
        loop {
            if state.failed {
                return None;
            }
            let mut missing = false;
            for key in keys {
                if !state.solids.contains_key(&key) {
                    missing = true;
                    if !state.wanted.contains(&key) {
                        state.queue_tile(key);
                    }
                }
            }
            if !missing {
                break;
            }
            self.shared.work.notify_one();
            if !state.solids.contains_key(&center) && behind >= state.cap * 2 {
                return None;
            }
            state = self
                .shared
                .filled
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
        let mut grid = SolidGrid::default();
        for key in keys {
            let held = state.solids.get_mut(&key).expect("held above");
            grid.insert(ChunkPos { x: key.0, z: key.1 }, Arc::clone(&held.solids));
            held.readers += 1;
            if held.readers >= READERS {
                state.solids.remove(&key);
            }
        }
        Some(grid)
    }
}

impl Drop for GpuSolidService {
    fn drop(&mut self) {
        self.shared.lock().stop = true;
        self.shared.work.notify_all();
        if let Some(dispatcher) = self.dispatcher.take() {
            let _ = dispatcher.join();
        }
    }
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn dispatch(gpu: &GpuDensity, shared: &Shared, max: usize) {
    loop {
        let batch: Vec<(i32, i32)> = {
            let mut state = shared.lock();
            while state.queued.is_empty() && !state.stop {
                state = shared
                    .work
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            if state.stop {
                return;
            }
            let take = state.queued.len().min(state.cap);
            state.queued.drain(..take).collect()
        };
        let origins: Vec<(i32, i32)> = batch.iter().map(|&(x, z)| (x * 16, z * 16)).collect();
        let start = Instant::now();
        let result = gpu.fill_chunks(&origins);
        let elapsed = start.elapsed();
        let mut state = shared.lock();
        match result {
            Ok(filled) => {
                for (key, solids) in batch.iter().zip(filled.chunks) {
                    state.wanted.remove(key);
                    let sequence = state.filled;
                    state.filled += 1;
                    state.solids.insert(
                        *key,
                        Held {
                            solids: Arc::new(solids),
                            readers: 0,
                            sequence,
                        },
                    );
                    state.order.push_back((*key, sequence));
                }
                let State { solids, order, .. } = &mut *state;
                let current = |solids: &HashMap<_, Held>, (key, sequence): &((i32, i32), u64)| {
                    solids
                        .get(key)
                        .is_some_and(|held| held.sequence == *sequence)
                };
                while solids.len() > CACHE_CHUNKS {
                    let Some(oldest) = order.pop_front() else {
                        break;
                    };
                    if current(solids, &oldest) {
                        solids.remove(&oldest.0);
                    }
                }
                if order.len() > 2 * CACHE_CHUNKS {
                    order.retain(|entry| current(solids, entry));
                }
                let cap = state.cap;
                state.cap = if elapsed > BATCH_BUDGET {
                    (cap / 2).max(1)
                } else if batch.len() == cap && elapsed < BATCH_BUDGET / 2 {
                    (cap * 2).min(max)
                } else {
                    cap
                };
                tracing::debug!(
                    chunks = batch.len(),
                    ms = elapsed.as_secs_f64() * 1000.0,
                    cap = state.cap,
                    queued = state.queued.len(),
                    "worldgen GPU batch"
                );
            }
            Err(error) => {
                tracing::warn!(%error, "worldgen GPU failed; density runs on the CPU from now on");
                state.failed = true;
            }
        }
        let failed = state.failed;
        drop(state);
        shared.filled.notify_all();
        if failed {
            return;
        }
    }
}
