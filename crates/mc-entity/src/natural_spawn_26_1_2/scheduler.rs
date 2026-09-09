use std::collections::HashSet;
use std::sync::Arc;

const NATURAL_SPAWN_METRIC_LOG_INTERVAL_TICKS: u64 = 1_200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NaturalSpawnCategory {
    Friendly,
    Hostile,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NaturalSpawnCategoryReport {
    pub attempts: u64,
    pub chunks_sampled: u64,
    pub templates_considered: u64,
    pub committed: u64,
    pub rejected_unloaded: u64,
    pub rejected_time: u64,
    pub rejected_player_distance: u64,
    pub rejected_block_or_fluid: u64,
    pub rejected_darkness: u64,
    pub rejected_collision: u64,
    pub rejected_duplicate_or_stale: u64,
}

impl NaturalSpawnCategoryReport {
    pub fn merge(&mut self, other: Self) {
        self.attempts = self.attempts.saturating_add(other.attempts);
        self.chunks_sampled = self.chunks_sampled.saturating_add(other.chunks_sampled);
        self.templates_considered = self
            .templates_considered
            .saturating_add(other.templates_considered);
        self.committed = self.committed.saturating_add(other.committed);
        self.rejected_unloaded = self
            .rejected_unloaded
            .saturating_add(other.rejected_unloaded);
        self.rejected_time = self.rejected_time.saturating_add(other.rejected_time);
        self.rejected_player_distance = self
            .rejected_player_distance
            .saturating_add(other.rejected_player_distance);
        self.rejected_block_or_fluid = self
            .rejected_block_or_fluid
            .saturating_add(other.rejected_block_or_fluid);
        self.rejected_darkness = self
            .rejected_darkness
            .saturating_add(other.rejected_darkness);
        self.rejected_collision = self
            .rejected_collision
            .saturating_add(other.rejected_collision);
        self.rejected_duplicate_or_stale = self
            .rejected_duplicate_or_stale
            .saturating_add(other.rejected_duplicate_or_stale);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NaturalSpawnReport {
    pub friendly: NaturalSpawnCategoryReport,
    pub hostile: NaturalSpawnCategoryReport,
}

impl NaturalSpawnReport {
    pub fn merge(&mut self, other: Self) {
        self.friendly.merge(other.friendly);
        self.hostile.merge(other.hostile);
    }
}

#[derive(Debug, Default)]
pub struct NaturalSpawnScheduler {
    active_snapshot: Option<Arc<HashSet<(i32, i32)>>>,
    active_ring: Vec<(i32, i32)>,
    player_chunks: Vec<(i32, i32)>,
    radius: u32,
    friendly_cursor: usize,
    hostile_cursor: usize,
    cumulative: NaturalSpawnReport,
    last_log_tick: u64,
}

impl NaturalSpawnScheduler {
    pub fn select_chunks(
        &mut self,
        category: NaturalSpawnCategory,
        active_chunks: &Arc<HashSet<(i32, i32)>>,
        player_chunks: &[(i32, i32)],
        radius: u32,
        chunk_budget: usize,
        eligible: impl Fn(&(i32, i32)) -> bool,
    ) -> Vec<(i32, i32)> {
        let unchanged = self
            .active_snapshot
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, active_chunks))
            && self.player_chunks == player_chunks
            && self.radius == radius;
        if !unchanged {
            self.active_ring.clear();
            let radius = radius.min(8) as i32;
            for &(cx, cz) in player_chunks {
                for z in cz - radius..=cz + radius {
                    for x in cx - radius..=cx + radius {
                        if active_chunks.contains(&(x, z)) {
                            self.active_ring.push((x, z));
                        }
                    }
                }
            }
            self.active_ring.sort_unstable_by_key(|&(cx, cz)| (cz, cx));
            self.active_ring.dedup();
            self.player_chunks.clear();
            self.player_chunks.extend_from_slice(player_chunks);
            self.radius = radius as u32;
            self.active_snapshot = Some(Arc::clone(active_chunks));
            if self.active_ring.is_empty() {
                self.friendly_cursor = 0;
                self.hostile_cursor = 0;
            } else {
                self.friendly_cursor %= self.active_ring.len();
                self.hostile_cursor %= self.active_ring.len();
            }
        }
        if self.active_ring.is_empty() {
            return Vec::new();
        }
        let cursor = match category {
            NaturalSpawnCategory::Friendly => &mut self.friendly_cursor,
            NaturalSpawnCategory::Hostile => &mut self.hostile_cursor,
        };
        let count = self.active_ring.len().min(chunk_budget);
        let mut selected = Vec::with_capacity(count);
        for _ in 0..self.active_ring.len().min(chunk_budget.saturating_mul(4)) {
            if selected.len() == count {
                break;
            }
            let chunk = self.active_ring[*cursor];
            *cursor = (*cursor + 1) % self.active_ring.len();
            if eligible(&chunk) {
                selected.push(chunk);
            }
        }
        selected
    }

    pub fn record(&mut self, tick: u64, report: NaturalSpawnReport) -> Option<NaturalSpawnReport> {
        self.cumulative.merge(report);
        if tick.saturating_sub(self.last_log_tick) < NATURAL_SPAWN_METRIC_LOG_INTERVAL_TICKS {
            return None;
        }
        self.last_log_tick = tick;
        Some(self.cumulative)
    }
}
