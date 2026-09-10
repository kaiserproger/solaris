use super::RuntimeTelemetryHandle;
use serde_json::{Value, json};

impl RuntimeTelemetryHandle {
    /// Expensive, explicitly requested operator capture; run on a blocking worker.
    /// Does not retain the world writer while traversing published chunk storage.
    pub fn resource_profile(&self) -> Value {
        let storage = self.profile_world.as_ref().map(|world| match world.try_lock() {
            Ok(world) => {
                let s = world.stats();
                json!({"available": true, "chunk_cache_len": s.chunk_cache_len,
                    "chunk_cache_capacity": s.chunk_cache_capacity, "resident_budget_bytes": s.resident_byte_budget,
                    "resident_reachable_estimate_bytes": s.resident_bytes, "dirty_chunks": s.dirty_chunks,
                    "dirty_reachable_estimate_bytes": s.dirty_bytes, "dirty_budget_bytes": s.dirty_byte_budget,
                    "region_cache_len": s.region_cache_len, "region_cache_capacity": s.region_cache_capacity,
                    "save_healthy": s.save_healthy, "dirty_saturated": s.dirty_chunk_cache_saturated,
                    "accounting": "dirty bytes are a subset of resident bytes; indexes retain no decompressed region NBT"})
            }
            Err(error) => json!({"available": false, "error": error.to_string()}),
        });
        let chunks = self.profile_read.as_ref().map(|read| {
            let p = read.memory_profile();
            json!({"chunks": p.chunks, "dirty_chunks": p.dirty_chunks,
                "unique_allocation_estimate_bytes": p.unique_allocated_bytes,
                "reachable_estimate_bytes": p.reachable_bytes,
                "shared_bytes_deduplicated": p.shared_bytes_deduplicated,
                "categories_bytes": p.categories, "block_sections_by_bits": p.section_bits,
                "light_layers": {"unknown":p.light_unknown_layers,"uniform":p.light_uniform_layers,"shared_array":p.light_shared_layers},
                "accounting": "capacity estimates over current published chunks, sharing deduplicated within this set; excludes allocator size classes/control bytes and older external snapshots; sequential shard capture"})
        });
        let workers = self.profile_resources.metrics().snapshot();
        let block_registry = self.profile_blocks.memory_profile();
        let block_registry_bytes = block_registry.values().sum::<usize>();
        json!({"world_storage":storage,"resident_chunks":chunks,
            "block_registry":{"estimated_bytes":block_registry_bytes,"categories_bytes":block_registry,"accounting":"shared block definitions charged once; capacities plus identifier lengths, excludes hash control bytes"},
            "network_and_entities":self.sessions.resource_memory_snapshot(),
            "light_workspaces":crate::resource_profile::light_workspaces(),
            "lock_pressure_wall_time": lock_snapshot(),
            "workers":{"active_io":workers.active_io,"active_cpu":workers.active_cpu,"max_active_io":workers.max_io_active,"max_active_cpu":workers.max_cpu_active,"max_result_queue_depth":workers.max_result_queue_depth},
            "coverage_gaps":["retired snapshots outside current publications","queued network payload backing allocations","ECS nested component allocations","plugin/native heaps","other registries and generator caches","transient in-flight encode/decode/save buffers"],
            "note":"Breakdowns are ownership estimates, not independently additive RSS measurements. Missing owners are explicit; do not label the remainder an allocator leak."})
    }
}

fn lock_snapshot() -> Value {
    let s = crate::lock_pressure_snapshot();
    let rows: Vec<_> = [
        ("world_storage", s.world_storage),
        ("session_registry", s.session_registry),
        ("container_registry", s.container_registry),
        ("save_all_flush", s.save_all_flush),
        ("chunk_prepare", s.chunk_prepare),
        ("player_persistence", s.player_persistence),
    ]
    .into_iter()
    .map(|(name, v)| {
        json!({
            "name": name, "wait_count": v.wait_count, "wait_us_total": v.wait_us,
            "max_wait_us": v.max_wait_us, "hold_count": v.hold_count,
            "hold_us_total": v.hold_us, "max_hold_us": v.max_hold_us,
        })
    })
    .collect();
    json!({"source":"existing lock timers; cumulative wall time, not CPU", "locks":rows})
}
