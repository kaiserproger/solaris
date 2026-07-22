use std::collections::HashMap;

use mc_world::ChunkPos;

const FLUSH_INTERVAL_TICKS: u64 = 20;

#[derive(Default)]
pub(crate) struct InhabitedTimeAccumulator {
    pending: HashMap<(i32, i32), PendingInhabitedTime>,
}

struct PendingInhabitedTime {
    elapsed: u64,
    last_seen_tick: u64,
}

impl InhabitedTimeAccumulator {
    pub(crate) fn observe_tick(
        &mut self,
        tick: u64,
        active_chunks: &[(i32, i32)],
    ) -> Vec<(ChunkPos, u64)> {
        for &position in active_chunks {
            let pending = self
                .pending
                .entry(position)
                .or_insert(PendingInhabitedTime {
                    elapsed: 0,
                    last_seen_tick: tick,
                });
            pending.elapsed = pending.elapsed.saturating_add(1);
            pending.last_seen_tick = tick;
        }
        if tick.is_multiple_of(FLUSH_INTERVAL_TICKS) {
            self.drain()
        } else {
            let inactive = self
                .pending
                .iter()
                .filter_map(|(&position, pending)| {
                    (pending.last_seen_tick < tick).then_some(position)
                })
                .collect::<Vec<_>>();
            self.take(inactive)
        }
    }

    pub(crate) fn restore(&mut self, updates: Vec<(ChunkPos, u64)>) {
        for (position, elapsed) in updates {
            let pending =
                self.pending
                    .entry((position.x, position.z))
                    .or_insert(PendingInhabitedTime {
                        elapsed: 0,
                        last_seen_tick: u64::MAX,
                    });
            pending.elapsed = pending.elapsed.saturating_add(elapsed);
            pending.last_seen_tick = u64::MAX;
        }
    }

    pub(crate) fn drain(&mut self) -> Vec<(ChunkPos, u64)> {
        let positions = self.pending.keys().copied().collect::<Vec<_>>();
        self.take(positions)
    }

    fn take(&mut self, positions: Vec<(i32, i32)>) -> Vec<(ChunkPos, u64)> {
        let mut updates = positions
            .into_iter()
            .filter_map(|position| {
                self.pending.remove(&position).map(|pending| {
                    (
                        ChunkPos {
                            x: position.0,
                            z: position.1,
                        },
                        pending.elapsed,
                    )
                })
            })
            .collect::<Vec<_>>();
        updates.sort_unstable_by_key(|(position, _)| (position.x, position.z));
        updates
    }
}
