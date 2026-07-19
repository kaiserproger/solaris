use std::sync::Arc;
use tokio::sync::watch;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct MemoryPressureSnapshot {
    pub used_mb: u64,
    pub limit_mb: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct MemoryPressureObservation {
    pub sample: MemoryPressureSnapshot,
    pub available: bool,
    pub failures: u64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct MemoryPressureHandle {
    inner: Arc<MemoryPressureState>,
}

#[derive(Debug)]
struct MemoryPressureState {
    observation: watch::Sender<MemoryPressureObservation>,
}

impl Default for MemoryPressureState {
    fn default() -> Self {
        let (observation, _) = watch::channel(MemoryPressureObservation::default());
        Self { observation }
    }
}

impl MemoryPressureHandle {
    #[must_use]
    pub(crate) fn observation(&self) -> MemoryPressureObservation {
        *self.inner.observation.borrow()
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<MemoryPressureObservation> {
        self.inner.observation.subscribe()
    }

    #[cfg(test)]
    pub(crate) fn with_sample(snapshot: MemoryPressureSnapshot) -> Self {
        let handle = Self::default();
        handle.store(snapshot);
        handle
    }

    #[cfg(test)]
    pub(crate) fn set_sample(&self, snapshot: MemoryPressureSnapshot) {
        self.store(snapshot);
    }

    #[cfg(test)]
    pub(crate) fn fail_sample_for_test(&self) {
        self.record_failure();
    }

    fn store(&self, snapshot: MemoryPressureSnapshot) {
        self.inner.observation.send_if_modified(|current| {
            let updated = MemoryPressureObservation {
                sample: snapshot,
                available: true,
                failures: current.failures,
            };
            if *current == updated {
                return false;
            }
            *current = updated;
            true
        });
    }

    fn record_failure(&self) {
        self.inner.observation.send_modify(|current| {
            current.available = false;
            current.failures = current.failures.saturating_add(1);
        });
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MemoryPressureSampler {
    requests: tokio::sync::mpsc::Sender<()>,
}

impl MemoryPressureSampler {
    pub(crate) fn request(&self) -> bool {
        self.requests.try_send(()).is_ok()
    }
}

pub(crate) fn spawn_memory_pressure_sampler(
    handle: MemoryPressureHandle,
) -> (MemoryPressureSampler, tokio::task::JoinHandle<()>) {
    spawn_memory_pressure_sampler_with(handle, system_memory_pressure)
}

fn spawn_memory_pressure_sampler_with<R>(
    handle: MemoryPressureHandle,
    mut read: R,
) -> (MemoryPressureSampler, tokio::task::JoinHandle<()>)
where
    R: FnMut() -> Option<MemoryPressureSnapshot> + Send + 'static,
{
    let (requests, mut receiver) = tokio::sync::mpsc::channel(1);
    let worker = tokio::task::spawn_blocking(move || {
        while receiver.blocking_recv().is_some() {
            match read() {
                Some(snapshot) => handle.store(snapshot),
                None => handle.record_failure(),
            }
        }
    });
    (MemoryPressureSampler { requests }, worker)
}

#[cfg(target_os = "linux")]
fn system_memory_pressure() -> Option<MemoryPressureSnapshot> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let process_used_mb = kib_to_mib(parse_linux_status_rss_kb(&status)?);
    if let Some(cgroup) = cgroup_memory_reading() {
        return Some(MemoryPressureSnapshot {
            used_mb: cgroup.used_mb.unwrap_or(process_used_mb),
            limit_mb: cgroup.limit_mb,
        });
    }

    let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
    Some(MemoryPressureSnapshot {
        used_mb: process_used_mb,
        limit_mb: meminfo_total_mb(&meminfo)?,
    })
}

#[cfg(not(target_os = "linux"))]
fn system_memory_pressure() -> Option<MemoryPressureSnapshot> {
    None
}

#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CgroupMemoryReading {
    used_mb: Option<u64>,
    limit_mb: u64,
}

#[cfg(target_os = "linux")]
fn cgroup_memory_reading() -> Option<CgroupMemoryReading> {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    cgroup_memory_reading_from(std::path::Path::new("/sys/fs/cgroup"), &cgroup)
}

#[cfg(target_os = "linux")]
fn cgroup_memory_reading_from(root: &std::path::Path, cgroup: &str) -> Option<CgroupMemoryReading> {
    cgroup_v2_memory_reading_from(root, cgroup)
        .or_else(|| cgroup_v1_memory_reading_from(root, cgroup))
}

#[cfg(target_os = "linux")]
fn cgroup_v2_memory_reading_from(
    root: &std::path::Path,
    cgroup: &str,
) -> Option<CgroupMemoryReading> {
    let relative = parse_cgroup_v2_path(cgroup)?;
    let relative = relative.strip_prefix('/').unwrap_or(relative);
    let directory = root.join(relative);
    let limit = std::fs::read_to_string(directory.join("memory.max")).ok()?;
    let limit_mb = parse_cgroup_memory_max_mb(&limit)?;
    let used_mb = std::fs::read_to_string(directory.join("memory.current"))
        .ok()
        .and_then(|value| parse_cgroup_memory_bytes(&value))
        .map(bytes_to_mib);
    Some(CgroupMemoryReading { used_mb, limit_mb })
}

#[cfg(target_os = "linux")]
fn cgroup_v1_memory_reading_from(
    root: &std::path::Path,
    cgroup: &str,
) -> Option<CgroupMemoryReading> {
    let relative = parse_cgroup_v1_memory_path(cgroup)?;
    let relative = relative.strip_prefix('/').unwrap_or(relative);
    let directory = root.join("memory").join(relative);
    let limit = std::fs::read_to_string(directory.join("memory.limit_in_bytes")).ok()?;
    let limit_mb = parse_cgroup_v1_memory_limit_mb(&limit)?;
    let used_mb = std::fs::read_to_string(directory.join("memory.usage_in_bytes"))
        .ok()
        .and_then(|value| parse_cgroup_memory_bytes(&value))
        .map(bytes_to_mib);
    Some(CgroupMemoryReading { used_mb, limit_mb })
}

#[cfg(target_os = "linux")]
fn parse_linux_status_rss_kb(status: &str) -> Option<u64> {
    meminfo_value_kb(status, "VmRSS:")
}

#[cfg(target_os = "linux")]
fn meminfo_total_mb(meminfo: &str) -> Option<u64> {
    meminfo_value_kb(meminfo, "MemTotal:")
        .map(kib_to_mib)
        .map(|limit| limit.max(1))
}

#[cfg(target_os = "linux")]
fn parse_cgroup_v2_path(cgroup: &str) -> Option<&str> {
    cgroup.lines().find_map(|line| {
        let mut parts = line.splitn(3, ':');
        let hierarchy = parts.next()?;
        let controllers = parts.next()?;
        let path = parts.next()?;
        if hierarchy == "0" && controllers.is_empty() {
            Some(path)
        } else {
            None
        }
    })
}

#[cfg(target_os = "linux")]
fn parse_cgroup_v1_memory_path(cgroup: &str) -> Option<&str> {
    cgroup.lines().find_map(|line| {
        let mut parts = line.splitn(3, ':');
        parts.next()?;
        let controllers = parts.next()?;
        let path = parts.next()?;
        controllers
            .split(',')
            .any(|controller| controller == "memory")
            .then_some(path)
    })
}

#[cfg(target_os = "linux")]
fn parse_cgroup_memory_max_mb(value: &str) -> Option<u64> {
    let value = value.trim();
    if value == "max" {
        return None;
    }
    let bytes = parse_cgroup_memory_bytes(value)?;
    Some(bytes_to_mib(bytes).max(1))
}

#[cfg(target_os = "linux")]
fn parse_cgroup_v1_memory_limit_mb(value: &str) -> Option<u64> {
    const UNBOUNDED_SENTINEL_MIN: u64 = 1 << 60;

    let bytes = parse_cgroup_memory_bytes(value)?;
    if bytes >= UNBOUNDED_SENTINEL_MIN {
        return None;
    }
    Some(bytes_to_mib(bytes).max(1))
}

#[cfg(target_os = "linux")]
fn parse_cgroup_memory_bytes(value: &str) -> Option<u64> {
    value.trim().parse::<u64>().ok()
}

#[cfg(target_os = "linux")]
fn meminfo_value_kb(meminfo: &str, key: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let rest = line.strip_prefix(key)?;
        rest.split_whitespace().next()?.parse::<u64>().ok()
    })
}

#[cfg(target_os = "linux")]
fn kib_to_mib(value: u64) -> u64 {
    value.div_ceil(1024)
}

#[cfg(target_os = "linux")]
fn bytes_to_mib(value: u64) -> u64 {
    value.div_ceil(1024 * 1024)
}

#[cfg(test)]
mod sampler_tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sampler_coalesces_requests_and_publishes_off_ticker() {
        let handle = MemoryPressureHandle::default();
        let mut changes = handle.subscribe();
        let calls = Arc::new(AtomicU64::new(0));
        let (entered_tx, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let reader_calls = Arc::clone(&calls);
        let reader_release = Arc::clone(&release_rx);
        let (sampler, worker) = spawn_memory_pressure_sampler_with(handle, move || {
            let call = reader_calls.fetch_add(1, Ordering::Relaxed) + 1;
            entered_tx.send(call).expect("sampler entry receiver");
            reader_release
                .lock()
                .expect("sampler release mutex poisoned")
                .recv()
                .expect("sampler release sender");
            Some(MemoryPressureSnapshot {
                used_mb: 100 + call,
                limit_mb: 1_000,
            })
        });

        assert!(sampler.request());
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), entered_rx.recv())
                .await
                .expect("first sample must start"),
            Some(1)
        );
        assert!(sampler.request());
        assert!(!sampler.request(), "busy sampler queue must stay bounded");

        release_tx.send(()).expect("release first sample");
        tokio::time::timeout(Duration::from_secs(1), changes.changed())
            .await
            .expect("first sample must publish")
            .expect("sample publisher remains alive");
        assert_eq!(changes.borrow_and_update().sample.used_mb, 101);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(1), entered_rx.recv())
                .await
                .expect("queued sample must start"),
            Some(2)
        );

        release_tx.send(()).expect("release queued sample");
        tokio::time::timeout(Duration::from_secs(1), changes.changed())
            .await
            .expect("queued sample must publish")
            .expect("sample publisher remains alive");
        assert_eq!(changes.borrow_and_update().sample.used_mb, 102);
        drop(sampler);
        worker.await.expect("sampler worker joins");
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sampler_failure_marks_last_sample_unavailable_and_wakes_subscriber() {
        let sample = MemoryPressureSnapshot {
            used_mb: 700,
            limit_mb: 1_000,
        };
        let handle = MemoryPressureHandle::with_sample(sample);
        let mut changes = handle.subscribe();
        let (entered, mut entered_rx) = tokio::sync::mpsc::unbounded_channel();
        let (sampler, worker) = spawn_memory_pressure_sampler_with(handle, move || {
            entered.send(()).expect("sampler entry receiver");
            None
        });

        assert!(sampler.request());
        entered_rx.recv().await.expect("failed sample should run");
        tokio::time::timeout(Duration::from_secs(1), changes.changed())
            .await
            .expect("failed sample must notify consumers")
            .expect("sample publisher remains alive");
        let observation = *changes.borrow_and_update();
        assert_eq!(observation.sample, sample);
        assert!(!observation.available);
        assert_eq!(observation.failures, 1);

        drop(sampler);
        worker.await.expect("sampler worker joins");
    }

    #[test]
    fn successful_sample_restores_availability_and_keeps_failure_count() {
        let handle = MemoryPressureHandle::default();
        handle.fail_sample_for_test();
        handle.set_sample(MemoryPressureSnapshot {
            used_mb: 400,
            limit_mb: 1_000,
        });

        assert_eq!(
            handle.observation(),
            MemoryPressureObservation {
                sample: MemoryPressureSnapshot {
                    used_mb: 400,
                    limit_mb: 1_000,
                },
                available: true,
                failures: 1,
            }
        );
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_sample_change_wakes_subscriber_once() {
        let handle = MemoryPressureHandle::with_sample(MemoryPressureSnapshot {
            used_mb: 900,
            limit_mb: 1_000,
        });
        let mut changes = handle.subscribe();

        handle.set_sample(MemoryPressureSnapshot {
            used_mb: 100,
            limit_mb: 1_000,
        });

        tokio::time::timeout(std::time::Duration::from_secs(1), changes.changed())
            .await
            .expect("changed memory sample must wake subscribers")
            .expect("memory sample producer remains alive");
        assert_eq!(
            changes.borrow_and_update().sample,
            MemoryPressureSnapshot {
                used_mb: 100,
                limit_mb: 1_000,
            }
        );

        handle.set_sample(MemoryPressureSnapshot {
            used_mb: 100,
            limit_mb: 1_000,
        });
        assert!(!changes.has_changed().expect("producer remains alive"));
    }

    #[test]
    fn linux_status_parser_reports_process_rss_mb() {
        let used_kb = parse_linux_status_rss_kb(
            "Name:\tsolaris\n\
             VmPeak:\t  204800 kB\n\
             VmRSS:\t   65536 kB\n",
        )
        .expect("rss parses");

        assert_eq!(kib_to_mib(used_kb), 64);
    }

    #[test]
    fn linux_meminfo_parser_reports_host_limit_mb() {
        let limit = meminfo_total_mb(
            "MemTotal:       1048576 kB\n\
             MemAvailable:     262144 kB\n",
        )
        .expect("meminfo limit parses");

        assert_eq!(limit, 1024);
    }

    #[test]
    fn cgroup_memory_max_parser_ignores_unbounded_limit() {
        assert_eq!(parse_cgroup_memory_max_mb("max\n"), None);
        assert_eq!(parse_cgroup_memory_max_mb("1073741824\n"), Some(1024));
    }

    #[test]
    fn cgroup_v2_path_parser_finds_unified_hierarchy() {
        assert_eq!(
            parse_cgroup_v2_path("12:cpu:/ignored\n0::/user.slice/session.scope\n"),
            Some("/user.slice/session.scope")
        );
    }

    #[test]
    fn cgroup_v2_reader_uses_group_usage_and_limit() {
        let root = tempfile::tempdir().unwrap();
        let group = root.path().join("tenant/server");
        std::fs::create_dir_all(&group).unwrap();
        std::fs::write(group.join("memory.current"), "314572800\n").unwrap();
        std::fs::write(group.join("memory.max"), "536870912\n").unwrap();

        assert_eq!(
            cgroup_memory_reading_from(root.path(), "0::/tenant/server\n"),
            Some(CgroupMemoryReading {
                used_mb: Some(300),
                limit_mb: 512,
            })
        );
    }

    #[test]
    fn cgroup_v2_reader_keeps_limit_when_usage_is_unavailable() {
        let root = tempfile::tempdir().unwrap();
        let group = root.path().join("tenant/server");
        std::fs::create_dir_all(&group).unwrap();
        std::fs::write(group.join("memory.max"), "536870912\n").unwrap();

        assert_eq!(
            cgroup_memory_reading_from(root.path(), "0::/tenant/server\n"),
            Some(CgroupMemoryReading {
                used_mb: None,
                limit_mb: 512,
            })
        );
    }

    #[test]
    fn cgroup_v1_reader_uses_memory_controller_files() {
        let root = tempfile::tempdir().unwrap();
        let group = root.path().join("memory/docker/container");
        std::fs::create_dir_all(&group).unwrap();
        std::fs::write(group.join("memory.usage_in_bytes"), "268435456\n").unwrap();
        std::fs::write(group.join("memory.limit_in_bytes"), "1073741824\n").unwrap();

        assert_eq!(
            cgroup_memory_reading_from(
                root.path(),
                "7:cpu,cpuacct:/docker/container\n5:memory:/docker/container\n",
            ),
            Some(CgroupMemoryReading {
                used_mb: Some(256),
                limit_mb: 1024,
            })
        );
    }

    #[test]
    fn cgroup_v1_reader_ignores_kernel_unbounded_sentinel() {
        let root = tempfile::tempdir().unwrap();
        let group = root.path().join("memory/docker/container");
        std::fs::create_dir_all(&group).unwrap();
        std::fs::write(group.join("memory.usage_in_bytes"), "268435456\n").unwrap();
        std::fs::write(group.join("memory.limit_in_bytes"), "9223372036854771712\n").unwrap();

        assert_eq!(
            cgroup_memory_reading_from(root.path(), "5:memory:/docker/container\n"),
            None
        );
    }
}
