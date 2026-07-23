use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

pub(crate) const DEFAULT_RUNTIME_TICK_METRICS_CAPACITY: usize = 1_200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RuntimeTickSample {
    pub(crate) tick_us: u64,
    pub(crate) world_time_us: u64,
    pub(crate) sheep_grazing_us: u64,
    pub(crate) animal_breeding_us: u64,
    pub(crate) hostile_attacks_us: u64,
    pub(crate) entity_goals_us: u64,
    pub(crate) entity_physics_us: u64,
    pub(crate) entity_dispatch_us: u64,
    pub(crate) campfire_tick_us: u64,
    pub(crate) entity_save_us: u64,
    pub(crate) random_tick_us: u64,
    pub(crate) block_tick_us: u64,
    pub(crate) fluid_tick_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeLatencyPercentiles {
    pub samples: usize,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub max_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeTickPercentiles {
    pub source_tick: u64,
    pub observer_submit_us: u64,
    pub observer_compute_us: u64,
    pub observer_skipped_windows: u64,
    pub tick: RuntimeLatencyPercentiles,
    pub world_time: RuntimeLatencyPercentiles,
    pub sheep_grazing: RuntimeLatencyPercentiles,
    pub animal_breeding: RuntimeLatencyPercentiles,
    pub hostile_attacks: RuntimeLatencyPercentiles,
    pub entity_goals: RuntimeLatencyPercentiles,
    pub entity_physics: RuntimeLatencyPercentiles,
    pub entity_dispatch: RuntimeLatencyPercentiles,
    pub campfire_tick: RuntimeLatencyPercentiles,
    pub entity_save: RuntimeLatencyPercentiles,
    pub random_tick: RuntimeLatencyPercentiles,
    pub block_tick: RuntimeLatencyPercentiles,
    pub fluid_tick: RuntimeLatencyPercentiles,
}

#[derive(Debug)]
struct RuntimeTickSnapshotRequest {
    source_tick: u64,
    observer_submit_us: u64,
    scheduled_budget_exhausted: bool,
    samples: Vec<RuntimeTickSample>,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RuntimeTickMetricsObservation {
    pub(crate) percentiles: RuntimeTickPercentiles,
    pub(crate) scheduled_budget_exhausted: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct RuntimeTickMetricsPublisher {
    requests: tokio::sync::mpsc::Sender<RuntimeTickSnapshotRequest>,
    skipped_windows: Arc<AtomicU64>,
}

impl RuntimeTickMetricsPublisher {
    pub(crate) fn try_publish(
        &self,
        source_tick: u64,
        window: &RuntimeTickMetricsWindow,
        scheduled_budget_exhausted: bool,
    ) -> bool {
        if window.samples.is_empty() {
            return false;
        }
        let permit = match self.requests.try_reserve() {
            Ok(permit) => permit,
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                self.skipped_windows.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => return false,
        };
        let started = Instant::now();
        let samples = window.samples.iter().copied().collect();
        let observer_submit_us = elapsed_us(started);
        permit.send(RuntimeTickSnapshotRequest {
            source_tick,
            observer_submit_us,
            scheduled_budget_exhausted,
            samples,
        });
        true
    }
}

pub(crate) fn spawn_runtime_tick_metrics_worker(
    handle: RuntimeTickMetricsHandle,
) -> (
    RuntimeTickMetricsPublisher,
    tokio::sync::mpsc::Receiver<RuntimeTickMetricsObservation>,
    tokio::task::JoinHandle<()>,
) {
    let (requests, mut receiver) = tokio::sync::mpsc::channel(1);
    let (observations, observation_receiver) = tokio::sync::mpsc::channel(1);
    let skipped_windows = Arc::new(AtomicU64::new(0));
    let publisher = RuntimeTickMetricsPublisher {
        requests,
        skipped_windows: Arc::clone(&skipped_windows),
    };
    let worker = tokio::spawn(async move {
        while let Some(request) = receiver.recv().await {
            let source_tick = request.source_tick;
            let observer_submit_us = request.observer_submit_us;
            let scheduled_budget_exhausted = request.scheduled_budget_exhausted;
            let computed = tokio::task::spawn_blocking(move || {
                let started = Instant::now();
                let snapshot = RuntimeTickMetricsWindow::snapshot_from_samples(&request.samples);
                (snapshot, elapsed_us(started))
            })
            .await;
            let Ok((Some(mut snapshot), observer_compute_us)) = computed else {
                continue;
            };
            snapshot.source_tick = source_tick;
            snapshot.observer_submit_us = observer_submit_us;
            snapshot.observer_compute_us = observer_compute_us;
            snapshot.observer_skipped_windows = skipped_windows.load(Ordering::Relaxed);
            handle.publish(snapshot);
            if observations
                .send(RuntimeTickMetricsObservation {
                    percentiles: snapshot,
                    scheduled_budget_exhausted,
                })
                .await
                .is_err()
            {
                break;
            }
        }
    });
    (publisher, observation_receiver, worker)
}

#[derive(Debug, Clone, Default)]
pub(crate) struct RuntimeTickMetricsHandle {
    latest: Arc<RwLock<Option<RuntimeTickPercentiles>>>,
}

impl RuntimeTickMetricsHandle {
    pub(crate) fn publish(&self, snapshot: RuntimeTickPercentiles) {
        *self
            .latest
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(snapshot);
    }

    pub(crate) fn snapshot(&self) -> Option<RuntimeTickPercentiles> {
        *self
            .latest
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug)]
pub(crate) struct RuntimeTickMetricsWindow {
    capacity: usize,
    samples: VecDeque<RuntimeTickSample>,
}

impl Default for RuntimeTickMetricsWindow {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_RUNTIME_TICK_METRICS_CAPACITY)
    }
}

impl RuntimeTickMetricsWindow {
    pub(crate) fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            samples: VecDeque::with_capacity(capacity),
        }
    }

    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }

    pub(crate) fn record(&mut self, sample: RuntimeTickSample) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Option<RuntimeTickPercentiles> {
        let samples = self.samples.iter().copied().collect::<Vec<_>>();
        Self::snapshot_from_samples(&samples)
    }

    fn snapshot_from_samples(samples: &[RuntimeTickSample]) -> Option<RuntimeTickPercentiles> {
        if samples.is_empty() {
            return None;
        }

        Some(RuntimeTickPercentiles {
            source_tick: 0,
            observer_submit_us: 0,
            observer_compute_us: 0,
            observer_skipped_windows: 0,
            tick: Self::percentiles(samples, |sample| sample.tick_us),
            world_time: Self::percentiles(samples, |sample| sample.world_time_us),
            sheep_grazing: Self::percentiles(samples, |sample| sample.sheep_grazing_us),
            animal_breeding: Self::percentiles(samples, |sample| sample.animal_breeding_us),
            hostile_attacks: Self::percentiles(samples, |sample| sample.hostile_attacks_us),
            entity_goals: Self::percentiles(samples, |sample| sample.entity_goals_us),
            entity_physics: Self::percentiles(samples, |sample| sample.entity_physics_us),
            entity_dispatch: Self::percentiles(samples, |sample| sample.entity_dispatch_us),
            campfire_tick: Self::percentiles(samples, |sample| sample.campfire_tick_us),
            entity_save: Self::percentiles(samples, |sample| sample.entity_save_us),
            random_tick: Self::percentiles(samples, |sample| sample.random_tick_us),
            block_tick: Self::percentiles(samples, |sample| sample.block_tick_us),
            fluid_tick: Self::percentiles(samples, |sample| sample.fluid_tick_us),
        })
    }

    fn percentiles(
        samples: &[RuntimeTickSample],
        value: impl Fn(&RuntimeTickSample) -> u64,
    ) -> RuntimeLatencyPercentiles {
        let mut values = samples.iter().map(value).collect::<Vec<_>>();
        values.sort_unstable();
        RuntimeLatencyPercentiles {
            samples: values.len(),
            p50_us: nearest_rank(&values, 50),
            p95_us: nearest_rank(&values, 95),
            p99_us: nearest_rank(&values, 99),
            max_us: *values.last().expect("non-empty runtime tick metric window"),
        }
    }
}

fn elapsed_us(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn nearest_rank(sorted_values: &[u64], percentile: usize) -> u64 {
    let rank = percentile
        .saturating_mul(sorted_values.len())
        .div_ceil(100)
        .clamp(1, sorted_values.len());
    sorted_values[rank - 1]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(base_us: u64) -> RuntimeTickSample {
        RuntimeTickSample {
            tick_us: base_us,
            world_time_us: base_us + 1,
            sheep_grazing_us: base_us + 2,
            animal_breeding_us: base_us + 3,
            hostile_attacks_us: base_us + 4,
            entity_goals_us: base_us + 5,
            entity_physics_us: base_us + 6,
            entity_dispatch_us: base_us + 7,
            campfire_tick_us: base_us + 8,
            entity_save_us: base_us + 9,
            random_tick_us: base_us + 10,
            block_tick_us: base_us + 11,
            fluid_tick_us: base_us + 12,
        }
    }

    #[test]
    fn known_distribution_uses_nearest_rank_percentiles() {
        let mut window = RuntimeTickMetricsWindow::with_capacity(100);
        for value in 1..=100 {
            window.record(sample(value));
        }

        let snapshot = window.snapshot().expect("recorded window");
        assert_eq!(snapshot.tick.samples, 100);
        assert_eq!(snapshot.tick.p50_us, 50);
        assert_eq!(snapshot.tick.p95_us, 95);
        assert_eq!(snapshot.tick.p99_us, 99);
        assert_eq!(snapshot.tick.max_us, 100);
    }

    #[test]
    fn bounded_window_evicts_oldest_sample() {
        let mut window = RuntimeTickMetricsWindow::with_capacity(3);
        for value in [10, 20, 30, 40] {
            window.record(sample(value));
        }

        let snapshot = window.snapshot().expect("recorded window");
        assert_eq!(snapshot.tick.samples, 3);
        assert_eq!(snapshot.tick.p50_us, 30);
        assert_eq!(snapshot.tick.p95_us, 40);
        assert_eq!(snapshot.tick.p99_us, 40);
        assert_eq!(snapshot.tick.max_us, 40);
    }

    #[test]
    fn snapshot_keeps_stage_measurements_independent() {
        let mut window = RuntimeTickMetricsWindow::with_capacity(1);
        window.record(sample(100));

        let snapshot = window.snapshot().expect("recorded window");
        assert_eq!(snapshot.tick.p50_us, 100);
        assert_eq!(snapshot.world_time.p50_us, 101);
        assert_eq!(snapshot.sheep_grazing.p50_us, 102);
        assert_eq!(snapshot.animal_breeding.p50_us, 103);
        assert_eq!(snapshot.hostile_attacks.p50_us, 104);
        assert_eq!(snapshot.entity_goals.p50_us, 105);
        assert_eq!(snapshot.entity_physics.p50_us, 106);
        assert_eq!(snapshot.entity_dispatch.p50_us, 107);
        assert_eq!(snapshot.campfire_tick.p50_us, 108);
        assert_eq!(snapshot.entity_save.p50_us, 109);
        assert_eq!(snapshot.random_tick.p50_us, 110);
        assert_eq!(snapshot.block_tick.p50_us, 111);
        assert_eq!(snapshot.fluid_tick.p50_us, 112);
    }

    #[test]
    fn empty_window_has_no_snapshot() {
        assert!(RuntimeTickMetricsWindow::default().snapshot().is_none());
    }

    #[test]
    fn handle_publishes_the_latest_percentile_snapshot() {
        let handle = RuntimeTickMetricsHandle::default();
        assert!(handle.snapshot().is_none());

        let mut first = RuntimeTickMetricsWindow::with_capacity(2);
        first.record(sample(10));
        first.record(sample(20));
        handle.publish(first.snapshot().expect("first window"));

        let published = handle.snapshot().expect("published first window");
        assert_eq!(published.tick.samples, 2);
        assert_eq!(published.tick.p95_us, 20);

        let mut second = RuntimeTickMetricsWindow::with_capacity(1);
        second.record(sample(30));
        handle.publish(second.snapshot().expect("second window"));

        assert_eq!(
            handle
                .snapshot()
                .expect("published second window")
                .tick
                .p50_us,
            30
        );
    }

    #[tokio::test]
    async fn worker_publishes_off_tick_snapshot_with_provenance() {
        let handle = RuntimeTickMetricsHandle::default();
        let (publisher, mut observations, worker) =
            spawn_runtime_tick_metrics_worker(handle.clone());
        let mut window = RuntimeTickMetricsWindow::with_capacity(2);
        window.record(sample(10));
        window.record(sample(20));

        assert!(publisher.try_publish(42, &window, true));
        let observation = observations.recv().await.expect("computed window pushed");
        assert_eq!(observation.percentiles.source_tick, 42);
        assert!(observation.scheduled_budget_exhausted);
        drop(publisher);
        worker.await.expect("runtime metrics worker joins");

        let published = handle.snapshot().expect("worker published snapshot");
        assert_eq!(published.source_tick, 42);
        assert_eq!(published.tick.samples, 2);
        assert_eq!(published.tick.p95_us, 20);
        assert_eq!(published.observer_skipped_windows, 0);
    }

    #[test]
    fn publisher_skips_instead_of_waiting_for_busy_worker() {
        let (requests, _receiver) = tokio::sync::mpsc::channel(1);
        let skipped_windows = Arc::new(AtomicU64::new(0));
        let publisher = RuntimeTickMetricsPublisher {
            requests,
            skipped_windows: Arc::clone(&skipped_windows),
        };
        let mut window = RuntimeTickMetricsWindow::with_capacity(1);
        window.record(sample(10));

        assert!(publisher.try_publish(1, &window, false));
        assert!(!publisher.try_publish(2, &window, false));
        assert_eq!(skipped_windows.load(Ordering::Relaxed), 1);
    }
}
