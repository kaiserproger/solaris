use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct MemoryPressureSnapshot {
    pub used_mb: u64,
    pub limit_mb: u64,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct MemoryPressureHandle {
    inner: Arc<MemoryPressureState>,
}

#[derive(Debug, Default)]
struct MemoryPressureState {
    used_mb: AtomicU64,
    limit_mb: AtomicU64,
}

impl MemoryPressureHandle {
    #[must_use]
    pub(crate) fn snapshot(&self) -> MemoryPressureSnapshot {
        MemoryPressureSnapshot {
            used_mb: self.inner.used_mb.load(Ordering::Relaxed),
            limit_mb: self.inner.limit_mb.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn refresh_from_system(&self) {
        if let Some(snapshot) = system_memory_pressure() {
            self.store(snapshot);
        }
    }

    #[cfg(test)]
    pub(crate) fn with_sample(snapshot: MemoryPressureSnapshot) -> Self {
        let handle = Self::default();
        handle.store(snapshot);
        handle
    }

    fn store(&self, snapshot: MemoryPressureSnapshot) {
        self.inner
            .used_mb
            .store(snapshot.used_mb, Ordering::Relaxed);
        self.inner
            .limit_mb
            .store(snapshot.limit_mb, Ordering::Relaxed);
    }
}

#[cfg(target_os = "linux")]
fn system_memory_pressure() -> Option<MemoryPressureSnapshot> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let used_kb = parse_linux_status_rss_kb(&status)?;
    let limit_mb = cgroup_v2_memory_limit_mb().or_else(|| {
        let meminfo = std::fs::read_to_string("/proc/meminfo").ok()?;
        meminfo_total_mb(&meminfo)
    })?;
    Some(MemoryPressureSnapshot {
        used_mb: kib_to_mib(used_kb),
        limit_mb,
    })
}

#[cfg(not(target_os = "linux"))]
fn system_memory_pressure() -> Option<MemoryPressureSnapshot> {
    None
}

#[cfg(target_os = "linux")]
fn cgroup_v2_memory_limit_mb() -> Option<u64> {
    let cgroup = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let relative = parse_cgroup_v2_path(&cgroup)?;
    let relative = relative.strip_prefix('/').unwrap_or(relative);
    let memory_max = std::path::Path::new("/sys/fs/cgroup")
        .join(relative)
        .join("memory.max");
    let value = std::fs::read_to_string(memory_max).ok()?;
    parse_cgroup_memory_max_mb(&value)
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
fn parse_cgroup_memory_max_mb(value: &str) -> Option<u64> {
    let value = value.trim();
    if value == "max" {
        return None;
    }
    let bytes = value.parse::<u64>().ok()?;
    Some(bytes_to_mib(bytes).max(1))
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

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
}
