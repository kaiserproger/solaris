//! Chunk solids shared by every thread generating chunks. A chunk reads its
//! own and its eight neighbours' solids (ore halo, tree footprint, surface
//! heights), so each chunk's density is evaluated once and then read from here
//! by all nine generations instead of being re-searched column by column.
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use mc_world::ChunkPos;

use super::solids::{ChunkSolids, SolidGrid};

/// Chunk coordinates.
pub(crate) type Key = (i32, i32);

/// Solids kept for later readers (12 KiB each in the overworld). A chunk
/// leaves earlier once all [`READERS`] generations that read it are done.
const CACHE_CHUNKS: usize = 4096;
/// Generations that read one chunk's solids: its own and its 8 neighbours'.
const READERS: u8 = 9;

/// `pos` and its eight neighbours.
pub(crate) fn ring(pos: ChunkPos) -> [Key; 9] {
    std::array::from_fn(|i| (pos.x + i as i32 % 3 - 1, pos.z + i as i32 / 3 - 1))
}

#[derive(Default)]
pub(crate) struct HeldSolids {
    held: HashMap<Key, Held>,
    /// Fill order of `held`, oldest first, for eviction. Entries whose
    /// sequence no longer matches were released or refilled since.
    order: VecDeque<(Key, u64)>,
    filled: u64,
}

struct Held {
    solids: Arc<ChunkSolids>,
    readers: u8,
    sequence: u64,
}

impl HeldSolids {
    pub(crate) fn contains(&self, key: &Key) -> bool {
        self.held.contains_key(key)
    }

    pub(crate) fn insert(&mut self, key: Key, solids: ChunkSolids) {
        let sequence = self.filled;
        self.filled += 1;
        self.held.insert(
            key,
            Held {
                solids: Arc::new(solids),
                readers: 0,
                sequence,
            },
        );
        self.order.push_back((key, sequence));
        let Self { held, order, .. } = self;
        let current = |held: &HashMap<Key, Held>, (key, sequence): &(Key, u64)| {
            held.get(key).is_some_and(|h| h.sequence == *sequence)
        };
        while held.len() > CACHE_CHUNKS {
            let Some(oldest) = order.pop_front() else {
                break;
            };
            if current(held, &oldest) {
                held.remove(&oldest.0);
            }
        }
        if order.len() > 2 * CACHE_CHUNKS {
            order.retain(|entry| current(held, entry));
        }
    }

    /// The grid of `keys` (all held) for one generation, counting it as a
    /// reader of each.
    pub(crate) fn read(&mut self, keys: &[Key; 9]) -> SolidGrid {
        let mut grid = SolidGrid::default();
        for key in keys {
            let held = self.held.get_mut(key).expect("every ring chunk is held");
            grid.insert(ChunkPos { x: key.0, z: key.1 }, Arc::clone(&held.solids));
            held.readers += 1;
            if held.readers >= READERS {
                self.held.remove(key);
            }
        }
        grid
    }
}

/// CPU fills: a generation fills the ring chunks nobody else is filling on its
/// own thread and waits for the rest.
#[derive(Default)]
pub(crate) struct SolidCache {
    state: Mutex<CacheState>,
    filled: Condvar,
}

#[derive(Default)]
struct CacheState {
    solids: HeldSolids,
    filling: HashSet<Key>,
}

impl SolidCache {
    pub(crate) fn around(
        &self,
        pos: ChunkPos,
        fill: impl Fn(ChunkPos) -> ChunkSolids,
    ) -> SolidGrid {
        let keys = ring(pos);
        let mut state = self.lock();
        loop {
            let mut claimed = Vec::new();
            for key in keys {
                if !state.solids.contains(&key) && state.filling.insert(key) {
                    claimed.push(key);
                }
            }
            if !claimed.is_empty() {
                drop(state);
                let claims = Claims {
                    cache: self,
                    keys: claimed,
                };
                let filled: Vec<_> = claims
                    .keys
                    .iter()
                    .map(|&key| (key, fill(ChunkPos { x: key.0, z: key.1 })))
                    .collect();
                state = self.lock();
                for (key, solids) in filled {
                    state.solids.insert(key, solids);
                }
                drop(state);
                drop(claims);
                state = self.lock();
                continue;
            }
            if keys.iter().all(|key| state.solids.contains(key)) {
                return state.solids.read(&keys);
            }
            state = self
                .filled
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn lock(&self) -> MutexGuard<'_, CacheState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Keys one generation is filling. Released (and waiters woken) when the
/// fills are stored or when a fill unwinds, so no waiter is left behind.
struct Claims<'a> {
    cache: &'a SolidCache,
    keys: Vec<Key>,
}

impl Drop for Claims<'_> {
    fn drop(&mut self) {
        let mut state = self.cache.lock();
        for key in &self.keys {
            state.filling.remove(key);
        }
        drop(state);
        self.cache.filled.notify_all();
    }
}
