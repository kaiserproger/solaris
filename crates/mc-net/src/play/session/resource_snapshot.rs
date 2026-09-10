//! Explicit operator profiling only; unavailable locks are reported, not waited on.
use super::{HerdSpawn, PlaySession, SessionRegistry};
use serde_json::{Value, json};
use std::collections::HashSet;

impl SessionRegistry {
    pub(crate) fn resource_memory_snapshot(&self) -> Value {
        let prepared = match self.prepared_cache.try_lock() {
            Ok(cache) => {
                let mut seen = HashSet::new();
                let mut payload_bytes = 0;
                let mut light_reachable = 0;
                for frame in cache.prepared.values() {
                    if seen.insert((frame.frame.as_ptr() as usize, frame.frame.len())) {
                        payload_bytes += frame.frame.len();
                    }
                    light_reachable += frame.light.as_ref().map_or(0, |v| v.estimated_heap_bytes());
                }
                json!({"available": true, "chunks": cache.prepared.len(), "in_flight": cache.prepared_in_flight.len(),
                    "framed_payload_bytes": payload_bytes, "light_reachable_bytes": light_reachable,
                    "pending_subscribers": cache.pending_subscriber_counts.values().sum::<usize>(),
                    "prewarm_entries": cache.prewarmed_prepared.len(),
                    "accounting": "frame slice lengths, not backing allocation capacities; light can share with world and is NOT additive"})
            }
            Err(error) => json!({"available": false, "error": error.to_string()}),
        };
        let sessions = match self.inner.try_lock() {
            Ok(inner) => {
                let mut owned =
                    inner.sessions.capacity() * std::mem::size_of::<(u64, PlaySession)>();
                let mut queued = 0;
                for session in inner.sessions.values() {
                    owned += session.name.capacity()
                        + session.dimension.capacity()
                        + (session.desired.capacity() + session.loaded.capacity())
                            * std::mem::size_of::<(i32, i32)>()
                        + session.visible_players.capacity() * std::mem::size_of::<u64>();
                    queued += session
                        .tx
                        .max_capacity()
                        .saturating_sub(session.tx.capacity());
                }
                let templates = inner
                    .natural_spawn_templates
                    .values()
                    .map(|v| v.capacity() * std::mem::size_of::<HerdSpawn>())
                    .sum::<usize>();
                json!({"available": true, "count": inner.sessions.len(), "view_set_and_identity_capacity_bytes": owned,
                    "natural_spawn_template_capacity_bytes": templates, "outbound_queued_or_reserved_slots": queued,
                    "entity_snapshot_count": inner.published_entity_snapshots.len(),
                    "entity_snapshot_table_capacity_bytes": inner.published_entity_snapshots.capacity() * std::mem::size_of::<(super::EntityId,super::ServerEntitySnapshot)>(),
                    "accounting": "partial capacity estimates; hash control bytes, nested entity data and queued command payloads not included"})
            }
            Err(error) => json!({"available": false, "error": error.to_string()}),
        };
        json!({"prepared_cache": prepared, "sessions": sessions})
    }
}
