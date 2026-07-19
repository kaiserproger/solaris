use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use tracing::warn;

const SLOW_WAIT_WARNING_US: u64 = 10_000;
const SLOW_HOLD_WARNING_US: u64 = 25_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockMetricKind {
    WorldStorage,
    SessionRegistry,
    ContainerRegistry,
    SaveAllFlush,
    ChunkPrepare,
    PlayerPersistence,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LockMetricSnapshot {
    pub wait_count: u64,
    pub wait_us: u64,
    pub max_wait_us: u64,
    pub hold_count: u64,
    pub hold_us: u64,
    pub max_hold_us: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LockMetricsSnapshot {
    pub world_storage: LockMetricSnapshot,
    pub session_registry: LockMetricSnapshot,
    pub container_registry: LockMetricSnapshot,
    pub save_all_flush: LockMetricSnapshot,
    pub chunk_prepare: LockMetricSnapshot,
    pub player_persistence: LockMetricSnapshot,
}

struct LockMetric {
    wait_count: AtomicU64,
    wait_us: AtomicU64,
    max_wait_us: AtomicU64,
    hold_count: AtomicU64,
    hold_us: AtomicU64,
    max_hold_us: AtomicU64,
}

impl LockMetric {
    const fn new() -> Self {
        Self {
            wait_count: AtomicU64::new(0),
            wait_us: AtomicU64::new(0),
            max_wait_us: AtomicU64::new(0),
            hold_count: AtomicU64::new(0),
            hold_us: AtomicU64::new(0),
            max_hold_us: AtomicU64::new(0),
        }
    }

    fn snapshot(&self) -> LockMetricSnapshot {
        LockMetricSnapshot {
            wait_count: self.wait_count.load(Ordering::Relaxed),
            wait_us: self.wait_us.load(Ordering::Relaxed),
            max_wait_us: self.max_wait_us.load(Ordering::Relaxed),
            hold_count: self.hold_count.load(Ordering::Relaxed),
            hold_us: self.hold_us.load(Ordering::Relaxed),
            max_hold_us: self.max_hold_us.load(Ordering::Relaxed),
        }
    }
}

static WORLD_STORAGE: LockMetric = LockMetric::new();
static SESSION_REGISTRY: LockMetric = LockMetric::new();
static CONTAINER_REGISTRY: LockMetric = LockMetric::new();
static SAVE_ALL_FLUSH: LockMetric = LockMetric::new();
static CHUNK_PREPARE: LockMetric = LockMetric::new();
static PLAYER_PERSISTENCE: LockMetric = LockMetric::new();

pub(crate) struct TimedGuard<G> {
    guard: G,
    _hold: LockHoldTimer,
}

pub(crate) fn timed_guard<G>(
    kind: LockMetricKind,
    operation: &'static str,
    wait_started: Instant,
    guard: G,
) -> TimedGuard<G> {
    let wait_us = elapsed_us(wait_started);
    record_wait(kind, wait_us);
    if wait_us >= SLOW_WAIT_WARNING_US {
        warn!(
            lock = kind.name(),
            operation, wait_us, "lock wait exceeded M39 budget"
        );
    }
    TimedGuard {
        guard,
        _hold: LockHoldTimer {
            kind,
            operation,
            hold_started: Instant::now(),
        },
    }
}

pub(crate) fn snapshot() -> LockMetricsSnapshot {
    LockMetricsSnapshot {
        world_storage: WORLD_STORAGE.snapshot(),
        session_registry: SESSION_REGISTRY.snapshot(),
        container_registry: CONTAINER_REGISTRY.snapshot(),
        save_all_flush: SAVE_ALL_FLUSH.snapshot(),
        chunk_prepare: CHUNK_PREPARE.snapshot(),
        player_persistence: PLAYER_PERSISTENCE.snapshot(),
    }
}

pub fn lock_pressure_snapshot() -> LockMetricsSnapshot {
    snapshot()
}

impl<G: Deref> Deref for TimedGuard<G> {
    type Target = G::Target;

    fn deref(&self) -> &Self::Target {
        self.guard.deref()
    }
}

impl<G: DerefMut> DerefMut for TimedGuard<G> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.guard.deref_mut()
    }
}

struct LockHoldTimer {
    kind: LockMetricKind,
    operation: &'static str,
    hold_started: Instant,
}

impl Drop for LockHoldTimer {
    fn drop(&mut self) {
        let hold_us = elapsed_us(self.hold_started);
        record_hold(self.kind, hold_us);
        if hold_us >= SLOW_HOLD_WARNING_US {
            warn!(
                lock = self.kind.name(),
                operation = self.operation,
                hold_us,
                "lock hold exceeded M39 budget"
            );
        }
    }
}

impl LockMetricKind {
    fn name(self) -> &'static str {
        match self {
            Self::WorldStorage => "world_storage",
            Self::SessionRegistry => "session_registry",
            Self::ContainerRegistry => "container_registry",
            Self::SaveAllFlush => "save_all_flush",
            Self::ChunkPrepare => "chunk_prepare",
            Self::PlayerPersistence => "player_persistence",
        }
    }

    fn metric(self) -> &'static LockMetric {
        match self {
            Self::WorldStorage => &WORLD_STORAGE,
            Self::SessionRegistry => &SESSION_REGISTRY,
            Self::ContainerRegistry => &CONTAINER_REGISTRY,
            Self::SaveAllFlush => &SAVE_ALL_FLUSH,
            Self::ChunkPrepare => &CHUNK_PREPARE,
            Self::PlayerPersistence => &PLAYER_PERSISTENCE,
        }
    }
}

fn record_wait(kind: LockMetricKind, wait_us: u64) {
    let metric = kind.metric();
    metric.wait_count.fetch_add(1, Ordering::Relaxed);
    metric.wait_us.fetch_add(wait_us, Ordering::Relaxed);
    update_max(&metric.max_wait_us, wait_us);
}

fn record_hold(kind: LockMetricKind, hold_us: u64) {
    let metric = kind.metric();
    metric.hold_count.fetch_add(1, Ordering::Relaxed);
    metric.hold_us.fetch_add(hold_us, Ordering::Relaxed);
    update_max(&metric.max_hold_us, hold_us);
}

fn update_max(target: &AtomicU64, value: u64) {
    let mut current = target.load(Ordering::Relaxed);
    while value > current {
        match target.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(next) => current = next,
        }
    }
}

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timed_guard_records_wait_and_hold_counts() {
        let before = snapshot().world_storage;
        {
            let _guard = timed_guard(
                LockMetricKind::WorldStorage,
                "test world lock",
                Instant::now(),
                (),
            );
        }
        let after = snapshot().world_storage;

        assert!(after.wait_count > before.wait_count);
        assert!(after.hold_count > before.hold_count);
    }

    #[test]
    fn snapshot_separates_lock_classes() {
        let before = snapshot().player_persistence;
        {
            let _guard = timed_guard(
                LockMetricKind::PlayerPersistence,
                "test player persistence lock",
                Instant::now(),
                (),
            );
        }
        let after = snapshot().player_persistence;

        assert!(after.wait_count > before.wait_count);
        assert!(after.hold_count > before.hold_count);
    }
}
