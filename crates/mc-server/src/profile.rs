//! Explicit operator captures, separate from the cheap dashboard poll.
use crate::dashboard::StatsPayload;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// Keep the system allocator; count requested Rust allocation bytes, not usable
// allocator size classes. This adds atomic counter operations, not stack tracing.
#[global_allocator]
static ALLOCATOR: &stats_alloc::StatsAlloc<std::alloc::System> = &stats_alloc::INSTRUMENTED_SYSTEM;

#[derive(Serialize)]
pub struct ProfileReport {
    pub profile_schema: u32,
    pub captured_unix_ms: u64,
    pub capture_wall_ms: u64,
    #[serde(flatten)]
    pub stats: StatsPayload,
    pub process: Value,
    pub cpu: Value,
    pub allocations: Value,
    pub resources: Value,
}

struct Previous {
    at: Instant,
    cpu_ns: Option<u64>,
    stages: Vec<mc_net::resource_profile::CpuStageSnapshot>,
    rss_bytes: Option<u64>,
    allocations: stats_alloc::Stats,
    threads: BTreeMap<u32, u64>,
}
pub(crate) struct ProfileSampler {
    previous: Mutex<Previous>,
}
impl ProfileSampler {
    pub(crate) fn new() -> Self {
        Self {
            previous: Mutex::new(Previous {
                stages: mc_net::resource_profile::cpu_stages(),
                allocations: ALLOCATOR.stats(),
                threads: BTreeMap::new(),
                rss_bytes: None,
                at: Instant::now(),
                cpu_ns: mc_net::resource_profile::process_cpu_ns(),
            }),
        }
    }

    pub(crate) fn capture(
        &self,
        stats: StatsPayload,
        resources: impl FnOnce() -> Value,
    ) -> ProfileReport {
        let started = Instant::now();
        let now = Instant::now();
        let cpu_ns = mc_net::resource_profile::process_cpu_ns();
        let stages = mc_net::resource_profile::cpu_stages();
        let allocations = ALLOCATOR.stats();
        let (mut process, threads) = process_snapshot();
        let mut previous = self.previous.lock().unwrap_or_else(|e| e.into_inner());
        let interval_ns = now.duration_since(previous.at).as_nanos() as u64;
        let rss_bytes = process["rss_bytes"].as_u64();
        process["rss_change_since_previous_profile_bytes"] = json!(
            rss_bytes
                .zip(previous.rss_bytes)
                .map(|(now, old)| i128::from(now) - i128::from(old))
        );
        let process_delta = cpu_ns
            .zip(previous.cpu_ns)
            .map(|(a, b)| a.saturating_sub(b));
        let mut rows: Vec<Value> = stages.iter().zip(&previous.stages).map(|(current, old)| {
            let delta = current.exclusive_cpu_ns.zip(old.exclusive_cpu_ns).map(|(a,b)| a.saturating_sub(b));
            json!({"stage":current.name,"cpu_ns_total":current.exclusive_cpu_ns,"cpu_ns_interval":delta,
                "executions_or_polls_interval":current.executions_or_polls.saturating_sub(old.executions_or_polls),
                "execution_wall_ns_interval":current.inclusive_execution_wall_ns.saturating_sub(old.inclusive_execution_wall_ns),
                "max_execution_cpu_ns":current.max_execution_cpu_ns})
        }).collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row["cpu_ns_interval"].as_u64().unwrap_or(0)));
        let attributed = rows
            .iter()
            .filter_map(|row| row["cpu_ns_interval"].as_u64())
            .sum::<u64>();
        if let Some(rows) = process.get_mut("threads").and_then(Value::as_array_mut) {
            for row in rows.iter_mut() {
                let tid = row["tid"].as_u64().unwrap_or(0) as u32;
                let delta = threads
                    .get(&tid)
                    .zip(previous.threads.get(&tid))
                    .map(|(a, b)| a.saturating_sub(*b));
                row["cpu_ns_interval"] = json!(delta);
            }
            rows.sort_by_key(|row| std::cmp::Reverse(row["cpu_ns_interval"].as_u64().unwrap_or(0)));
        }
        let live = allocations
            .bytes_allocated
            .checked_sub(allocations.bytes_deallocated);
        let mut allocation_report = json!({"rust_requested_live_bytes":live,
            "rust_requested_live_change_interval_bytes":live.zip(previous.allocations.bytes_allocated.checked_sub(previous.allocations.bytes_deallocated)).map(|(now,old)| now as i128 - old as i128),
            "allocated_bytes_total":allocations.bytes_allocated,"freed_bytes_total":allocations.bytes_deallocated,
            "allocated_bytes_interval":allocations.bytes_allocated.saturating_sub(previous.allocations.bytes_allocated),
            "freed_bytes_interval":allocations.bytes_deallocated.saturating_sub(previous.allocations.bytes_deallocated),
            "allocations_interval":allocations.allocations.saturating_sub(previous.allocations.allocations),
            "reallocations_interval":allocations.reallocations.saturating_sub(previous.allocations.reallocations),
            "allocator_retained_bytes":null,
            "accounting":"System allocator requested sizes, includes realloc growth/shrink. Concurrent counter reads are not transactional. Excludes native allocations bypassing Rust and allocator overhead; RSS minus live bytes is NOT an exact retained/free-heap measurement."});
        *previous = Previous {
            at: now,
            cpu_ns,
            stages,
            allocations,
            threads,
            rss_bytes,
        };
        drop(previous);
        let resources = resources();
        allocation_report["component_reconciliation"] = allocation_coverage(live, &resources);
        ProfileReport {
            profile_schema: 1,
            captured_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            capture_wall_ms: started.elapsed().as_millis() as u64,
            stats,
            process,
            cpu: json!({"interval_ns":interval_ns,"process_cpu_ns_total":cpu_ns,"process_cpu_ns_interval":process_delta,
                "average_cores_used":process_delta.map(|v| v as f64 / interval_ns.max(1) as f64),
                "attributed_cpu_ns_interval":attributed,
                "unattributed_cpu_ns_interval":process_delta.map(|v| v.saturating_sub(attributed)),
                "completed_scope_boundary_skew_ns":process_delta.map(|v| attributed.saturating_sub(v)),
                "stages":rows,
                "accounting":"Exclusive thread CPU per synchronous scope or async poll. Wait/suspension excluded. Wall execution counters are inclusive and not additive. Completed scopes can straddle capture boundaries. First interval starts when the provider is created; later intervals are between profile captures. Uninstrumented/native/background work remains explicit."}),
            allocations: allocation_report,
            resources,
        }
    }
}

fn allocation_coverage(live: Option<usize>, resources: &Value) -> Value {
    let owners = [
        ("block_registry", "/block_registry/estimated_bytes"),
        (
            "published_chunks",
            "/resident_chunks/unique_allocation_estimate_bytes",
        ),
        (
            "light_worker_scratch",
            "/light_workspaces/allocated_capacity_bytes",
        ),
        (
            "prepared_frame_payloads",
            "/network_and_entities/prepared_cache/framed_payload_bytes",
        ),
        (
            "session_views",
            "/network_and_entities/sessions/view_set_and_identity_capacity_bytes",
        ),
        (
            "natural_spawn_templates",
            "/network_and_entities/sessions/natural_spawn_template_capacity_bytes",
        ),
        (
            "entity_snapshot_table",
            "/network_and_entities/sessions/entity_snapshot_table_capacity_bytes",
        ),
    ];
    let mut rows: Vec<_> = owners.into_iter().map(|(owner, path)| {
        json!({"owner":owner,"estimated_bytes":resources.pointer(path).and_then(Value::as_u64)})
    }).collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row["estimated_bytes"].as_u64().unwrap_or(0)));
    let captured = rows
        .iter()
        .filter_map(|row| row["estimated_bytes"].as_u64())
        .sum::<u64>();
    json!({"owners":rows,"captured_component_estimate_bytes":captured,
        "unattributed_rust_requested_bytes_estimate":live.map(|v| (v as u64).saturating_sub(captured)),
        "estimate_excess_over_rust_requested_bytes":live.map(|v| captured.saturating_sub(v as u64)),
        "accounting":"Observed capacities/slice lengths versus requested allocation bytes, not exact allocator attribution. Missing owners and old snapshots stay unattributed; estimates are sequential and can skew during mutations. Prepared-light reachable bytes are deliberately excluded because they can share resident arrays."})
}

fn memory_fields(text: &str) -> BTreeMap<String, u64> {
    text.lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            let mut words = value.split_whitespace();
            let number = words.next()?.parse::<u64>().ok()?;
            (words.next() == Some("kB")).then(|| (name.to_owned(), number.saturating_mul(1024)))
        })
        .collect()
}

fn process_snapshot() -> (Value, BTreeMap<u32, u64>) {
    let mut errors = Vec::new();
    let status = match std::fs::read_to_string("/proc/self/status") {
        Ok(text) => memory_fields(&text),
        Err(error) => {
            errors.push(format!("status: {error}"));
            BTreeMap::new()
        }
    };
    let smaps = match std::fs::read_to_string("/proc/self/smaps_rollup") {
        Ok(text) => memory_fields(&text),
        Err(error) => {
            errors.push(format!("smaps_rollup: {error}"));
            BTreeMap::new()
        }
    };
    let io = std::fs::read_to_string("/proc/self/io").ok().map(|text| {
        text.lines()
            .filter_map(|line| {
                let (key, value) = line.split_once(':')?;
                Some((key.to_owned(), value.trim().parse::<u64>().ok()?))
            })
            .collect::<BTreeMap<_, _>>()
    });
    let mut thread_rows = Vec::new();
    let mut threads = BTreeMap::new();
    match std::fs::read_dir("/proc/self/task") {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(tid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|v| v.parse::<u32>().ok())
                else {
                    continue;
                };
                let Ok(schedstat) = std::fs::read_to_string(path.join("schedstat")) else {
                    continue;
                };
                let values: Vec<u64> = schedstat
                    .split_whitespace()
                    .filter_map(|v| v.parse().ok())
                    .collect();
                if values.len() != 3 {
                    continue;
                }
                let name = std::fs::read_to_string(path.join("comm")).unwrap_or_default();
                threads.insert(tid, values[0]);
                thread_rows.push(json!({"tid":tid,"name":name.trim_end(),"cpu_ns_total":values[0],"runnable_wait_ns_total":values[1],"timeslices_total":values[2]}));
            }
        }
        Err(error) => errors.push(format!("threads: {error}")),
    }
    (
        json!({"pid":std::process::id(),"rss_bytes":status.get("VmRSS"),"anonymous_rss_bytes":status.get("RssAnon"),
        "file_rss_bytes":status.get("RssFile"),"shared_rss_bytes":status.get("RssShmem"),"peak_rss_bytes":status.get("VmHWM"),
        "virtual_bytes":status.get("VmSize"),"swap_bytes":status.get("VmSwap"),"smaps_bytes":smaps,"io_counters":io,
        "threads":thread_rows,"errors":errors,"source":"Linux procfs; missing measurements are null/errors, not zero. Thread intervals omit exited threads; process CPU includes them."}),
        threads,
    )
}
