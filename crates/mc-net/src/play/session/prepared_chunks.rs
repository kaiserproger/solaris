use super::*;
use std::collections::hash_map::Entry;

impl SessionRegistry {
    pub(super) fn lock_prepared_cache(
        &self,
        operation: &'static str,
    ) -> MutexGuard<'_, PreparedChunkCache> {
        self.prepared_cache.lock().unwrap_or_else(|poisoned| {
            warn!(
                operation,
                "prepared chunk cache mutex was poisoned; recovering state"
            );
            poisoned.into_inner()
        })
    }

    pub(super) fn publish_prepared_cache(&self, cache: &PreparedChunkCache) {
        self.pressure_observation
            .prepared_chunks
            .store(cache.prepared.len(), Ordering::Relaxed);
    }

    pub(super) fn update_prewarm_frontier_for_pose_locked(
        &self,
        inner: &SessionRegistryInner,
        id: SessionId,
        old_frontier: ((i32, i32), i32, f32),
    ) {
        let Some(session) = inner.sessions.get(&id) else {
            return;
        };
        let mut cache = self.lock_prepared_cache("update prepared cache prewarm frontier");
        remove_prewarm_frontier_locked(&mut cache, old_frontier.0, old_frontier.1, old_frontier.2);
        add_prewarm_frontier_locked(
            &mut cache,
            session.center,
            session.view_distance,
            session.pose.yaw,
        );
    }

    pub(in crate::play) fn prepared_chunk_or_claim(
        &self,
        chunk: (i32, i32),
    ) -> PreparedChunkClaimResult {
        #[cfg(test)]
        self.prepared_claim_calls.fetch_add(1, Ordering::Relaxed);

        let mut cache = self.lock_prepared_cache("prepared chunk claim");
        if cache.prepared.contains_key(&chunk) {
            touch_prewarmed_prepared_locked(&mut cache, chunk);
            return PreparedChunkClaimResult::Cached;
        }
        if cache.prepared_in_flight.contains_key(&chunk) {
            return PreparedChunkClaimResult::InFlight;
        }

        cache.next_prepared_claim = cache.next_prepared_claim.wrapping_add(1).max(1);
        let claim = PreparedChunkClaim {
            id: cache.next_prepared_claim,
            revision: cache.prepared_revisions.get(&chunk).copied().unwrap_or(0),
        };
        cache.prepared_in_flight.insert(chunk, claim);
        PreparedChunkClaimResult::Claimed(claim)
    }

    pub(in crate::play) fn prepared_change_generation(&self) -> u64 {
        self.prepared_change_generation.load(Ordering::Acquire)
    }

    pub(in crate::play) async fn wait_for_prepared_change(&self, observed: u64) {
        loop {
            let changed = self.prepared_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.prepared_change_generation() != observed {
                return;
            }
            changed.await;
        }
    }

    pub(super) fn mark_prepared_changed(&self) {
        self.prepared_change_generation
            .fetch_add(1, Ordering::Release);
        self.prepared_changed.notify_waiters();
    }

    pub(in crate::play) fn prepared_chunk_or_wait_for_earlier_session(
        &self,
        chunk: (i32, i32),
        session_id: SessionId,
    ) -> SessionPreparedChunkClaimResult {
        #[cfg(test)]
        self.prepared_claim_calls.fetch_add(1, Ordering::Relaxed);

        let inner = self.lock_inner("prepared chunk claim");
        let mut cache = self.lock_prepared_cache("prepared chunk cache claim");
        if let Some(prepared) = cache.prepared.get(&chunk).cloned() {
            let revision = cache.prepared_revisions.get(&chunk).copied().unwrap_or(0);
            touch_prewarmed_prepared_locked(&mut cache, chunk);
            return SessionPreparedChunkClaimResult::Cached(prepared, revision);
        }
        if earlier_session_pending_chunk_locked(&inner, chunk, session_id) {
            return SessionPreparedChunkClaimResult::WaitingForEarlierSession;
        }
        if cache.prepared_in_flight.contains_key(&chunk) {
            return SessionPreparedChunkClaimResult::InFlight;
        }

        cache.next_prepared_claim = cache.next_prepared_claim.wrapping_add(1).max(1);
        let claim = PreparedChunkClaim {
            id: cache.next_prepared_claim,
            revision: cache.prepared_revisions.get(&chunk).copied().unwrap_or(0),
        };
        cache.prepared_in_flight.insert(chunk, claim);
        SessionPreparedChunkClaimResult::Claimed(claim)
    }

    #[cfg(test)]
    pub(in crate::play) fn prepared_chunk_claim_call_count(&self) -> u64 {
        self.prepared_claim_calls.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(in crate::play) fn prepared_chunk(
        &self,
        chunk: (i32, i32),
    ) -> Option<Arc<PreparedChunkFrame>> {
        let cache = self.lock_prepared_cache("prepared chunk lookup");
        cache.prepared.get(&chunk).cloned()
    }

    pub(in crate::play) fn release_prepared_chunk_claim(
        &self,
        chunk: (i32, i32),
        claim: PreparedChunkClaim,
    ) -> bool {
        let should_notify = {
            let mut cache = self.lock_prepared_cache("release prepared chunk claim");
            if cache.prepared_in_flight.get(&chunk).copied() != Some(claim) {
                return false;
            }
            cache.prepared_in_flight.remove(&chunk);
            cleanup_prepared_revision_locked(&mut cache, chunk);
            let should_notify = !cache.prepared.contains_key(&chunk);
            self.publish_prepared_cache(&cache);
            should_notify
        };
        if should_notify {
            self.mark_prepared_changed();
        }
        true
    }

    pub(in crate::play) fn prepared_revision_is_current(
        &self,
        chunk: (i32, i32),
        revision: u64,
    ) -> bool {
        let cache = self.lock_prepared_cache("check prepared chunk revision");
        cache.prepared_revisions.get(&chunk).copied().unwrap_or(0) == revision
    }

    pub(in crate::play) fn cache_prepared_chunk_if_current(
        &self,
        chunk: (i32, i32),
        revision: u64,
        prepared: Arc<PreparedChunkFrame>,
    ) -> bool {
        let inserted = {
            let mut cache = self.lock_prepared_cache("cache prepared chunk");
            let inserted = if cache.prepared_revisions.get(&chunk).copied().unwrap_or(0) != revision
                || !chunk_has_pending_subscriber_locked(&cache, chunk)
            {
                false
            } else {
                match cache.prepared.entry(chunk) {
                    Entry::Occupied(_) => false,
                    Entry::Vacant(entry) => {
                        entry.insert(prepared);
                        true
                    }
                }
            };
            self.publish_prepared_cache(&cache);
            inserted
        };
        if inserted {
            self.mark_prepared_changed();
        }
        inserted
    }

    #[cfg(test)]
    pub(in crate::play) fn cache_prepared_chunk(
        &self,
        chunk: (i32, i32),
        prepared: Arc<PreparedChunkFrame>,
    ) {
        let revision = {
            let cache = self.lock_prepared_cache("lookup prepared chunk revision");
            cache.prepared_revisions.get(&chunk).copied().unwrap_or(0)
        };
        assert!(self.cache_prepared_chunk_if_current(chunk, revision, prepared));
    }

    pub(in crate::play) fn cache_prewarmed_chunk(
        &self,
        chunk: (i32, i32),
        revision: u64,
        prepared: Arc<PreparedChunkFrame>,
        limit: usize,
    ) -> bool {
        let inserted = {
            let mut cache = self.lock_prepared_cache("cache prewarmed chunk");
            if cache.prepared_revisions.get(&chunk).copied().unwrap_or(0) != revision {
                return false;
            }
            let inserted = match cache.prepared.entry(chunk) {
                Entry::Occupied(_) => false,
                Entry::Vacant(entry) => {
                    entry.insert(prepared);
                    true
                }
            };
            if !touch_prewarmed_prepared_locked(&mut cache, chunk) {
                cache.prewarmed_prepared.push_back(chunk);
            }
            while cache.prewarmed_prepared.len() > limit.max(1) {
                let eviction_index = cache
                    .prewarmed_prepared
                    .iter()
                    .position(|cached| !cache.prewarm_frontier_counts.contains_key(cached))
                    .unwrap_or(0);
                let Some(evicted) = cache.prewarmed_prepared.remove(eviction_index) else {
                    break;
                };
                if !cache.ticket_counts.contains_key(&evicted) {
                    cache.prepared.remove(&evicted);
                    cleanup_prepared_revision_locked(&mut cache, evicted);
                }
            }
            self.publish_prepared_cache(&cache);
            inserted
        };
        if inserted {
            self.mark_prepared_changed();
        }
        inserted
    }

    pub(in crate::play) fn invalidate_prepared_chunks(&self, chunks: &HashSet<(i32, i32)>) {
        if chunks.is_empty() {
            return;
        }
        {
            let mut cache = self.lock_prepared_cache("invalidate prepared chunks");
            cache
                .prewarmed_prepared
                .retain(|chunk| !chunks.contains(chunk));
            for chunk in chunks {
                cache.prepared.remove(chunk);
                let revision = cache.prepared_revisions.entry(*chunk).or_default();
                *revision = revision.wrapping_add(1).max(1);
                cleanup_prepared_revision_locked(&mut cache, *chunk);
            }
            self.publish_prepared_cache(&cache);
        }
        self.mark_prepared_changed();
    }

    pub(crate) fn shed_prepared_chunks(&self) -> usize {
        let removed = {
            let mut cache = self.lock_prepared_cache("shed prepared chunks");
            let removed = std::mem::take(&mut cache.prepared);
            cache.prewarmed_prepared.clear();
            for chunk in removed.keys().copied() {
                cleanup_prepared_revision_locked(&mut cache, chunk);
            }
            self.publish_prepared_cache(&cache);
            removed
        };
        let removed_count = removed.len();
        drop(removed);
        if removed_count > 0 {
            self.mark_prepared_changed();
        }
        removed_count
    }
}

fn cleanup_prepared_revision_locked(cache: &mut PreparedChunkCache, chunk: (i32, i32)) {
    if !cache.ticket_counts.contains_key(&chunk)
        && !cache.prepared.contains_key(&chunk)
        && !cache.prepared_in_flight.contains_key(&chunk)
        && !cache.prewarmed_prepared.contains(&chunk)
    {
        cache.prepared_revisions.remove(&chunk);
    }
}

fn touch_prewarmed_prepared_locked(cache: &mut PreparedChunkCache, chunk: (i32, i32)) -> bool {
    let Some(index) = cache
        .prewarmed_prepared
        .iter()
        .position(|cached| *cached == chunk)
    else {
        return false;
    };
    let cached = cache
        .prewarmed_prepared
        .remove(index)
        .expect("prewarm cache index came from the same queue");
    cache.prewarmed_prepared.push_back(cached);
    true
}

fn increment_prepared_count(counts: &mut HashMap<(i32, i32), usize>, chunk: (i32, i32)) {
    *counts.entry(chunk).or_default() += 1;
}

fn decrement_prepared_count(counts: &mut HashMap<(i32, i32), usize>, chunk: (i32, i32)) {
    let Some(count) = counts.get_mut(&chunk) else {
        debug_assert!(false, "prepared cache count is missing {chunk:?}");
        return;
    };
    debug_assert!(*count > 0, "prepared cache count is zero for {chunk:?}");
    *count = count.saturating_sub(1);
    if *count == 0 {
        counts.remove(&chunk);
    }
}

pub(super) fn add_prepared_ticket_locked(
    cache: &mut PreparedChunkCache,
    chunk: (i32, i32),
    pending: bool,
) {
    increment_prepared_count(&mut cache.ticket_counts, chunk);
    if pending {
        add_pending_prepared_subscriber_locked(cache, chunk);
    }
}

pub(super) fn remove_prepared_ticket_locked(
    cache: &mut PreparedChunkCache,
    chunk: (i32, i32),
    pending: bool,
) {
    decrement_prepared_count(&mut cache.ticket_counts, chunk);
    if pending {
        remove_pending_prepared_subscriber_locked(cache, chunk);
    }
    prune_prepared_cache_chunk_locked(cache, chunk);
}

pub(super) fn add_pending_prepared_subscriber_locked(
    cache: &mut PreparedChunkCache,
    chunk: (i32, i32),
) {
    increment_prepared_count(&mut cache.pending_subscriber_counts, chunk);
}

pub(super) fn remove_pending_prepared_subscriber_locked(
    cache: &mut PreparedChunkCache,
    chunk: (i32, i32),
) {
    decrement_prepared_count(&mut cache.pending_subscriber_counts, chunk);
}

pub(super) fn add_prewarm_frontier_locked(
    cache: &mut PreparedChunkCache,
    center: (i32, i32),
    view_distance: i32,
    yaw: f32,
) {
    for chunk in prepared_prewarm_frontier(center, view_distance, yaw) {
        increment_prepared_count(&mut cache.prewarm_frontier_counts, chunk);
    }
}

pub(super) fn remove_prewarm_frontier_locked(
    cache: &mut PreparedChunkCache,
    center: (i32, i32),
    view_distance: i32,
    yaw: f32,
) {
    for chunk in prepared_prewarm_frontier(center, view_distance, yaw) {
        decrement_prepared_count(&mut cache.prewarm_frontier_counts, chunk);
    }
}

fn prepared_prewarm_frontier(center: (i32, i32), view_distance: i32, yaw: f32) -> Vec<(i32, i32)> {
    let radius = view_distance.clamp(0, crate::MAX_VIEW_DISTANCE) + 1;
    if center.0.checked_sub(radius).is_none()
        || center.0.checked_add(radius).is_none()
        || center.1.checked_sub(radius).is_none()
        || center.1.checked_add(radius).is_none()
    {
        return Vec::new();
    }
    super::chunk_stream::prewarm_edge_ring_chunks(center.0, center.1, view_distance, yaw)
}

pub(super) fn prune_prepared_cache_locked(cache: &mut PreparedChunkCache) {
    let stale = cache
        .prepared
        .keys()
        .copied()
        .filter(|chunk| {
            !cache.ticket_counts.contains_key(chunk) && !cache.prewarmed_prepared.contains(chunk)
        })
        .collect::<Vec<_>>();
    for chunk in stale {
        prune_prepared_cache_chunk_locked(cache, chunk);
    }
}

fn prune_prepared_cache_chunk_locked(cache: &mut PreparedChunkCache, chunk: (i32, i32)) {
    if !cache.ticket_counts.contains_key(&chunk) && !cache.prewarmed_prepared.contains(&chunk) {
        cache.prepared.remove(&chunk);
        cleanup_prepared_revision_locked(cache, chunk);
    }
}

pub(super) fn evict_delivered_prepared_chunk_locked(
    cache: &mut PreparedChunkCache,
    chunk: (i32, i32),
) -> bool {
    if !cache.prepared.contains_key(&chunk) {
        return false;
    }
    if chunk_has_pending_subscriber_locked(cache, chunk) {
        return false;
    }
    if cache.prewarmed_prepared.contains(&chunk) {
        return false;
    }

    cache.prepared.remove(&chunk);
    cache.prewarmed_prepared.retain(|cached| *cached != chunk);
    cleanup_prepared_revision_locked(cache, chunk);
    true
}

fn chunk_has_pending_subscriber_locked(cache: &PreparedChunkCache, chunk: (i32, i32)) -> bool {
    cache.pending_subscriber_counts.contains_key(&chunk)
}

fn earlier_session_pending_chunk_locked(
    inner: &SessionRegistryInner,
    chunk: (i32, i32),
    session_id: SessionId,
) -> bool {
    inner.tickets.get(&chunk).is_some_and(|subscribers| {
        subscribers.iter().any(|other_id| {
            *other_id < session_id
                && inner
                    .sessions
                    .get(other_id)
                    .is_some_and(|session| !session.loaded.contains(&chunk))
        })
    })
}
