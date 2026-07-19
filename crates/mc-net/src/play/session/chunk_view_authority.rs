use std::collections::HashSet;
use std::sync::Arc;

use mc_nbt::Tag;
use mc_world::BlockPos;
use tracing::debug;

use super::outbound::{OutboundCommand, SessionRecipient, VisibilityDispatch};
use super::prepared_chunks::{
    add_pending_prepared_subscriber_locked, add_prepared_ticket_locked,
    add_prewarm_frontier_locked, evict_delivered_prepared_chunk_locked,
    prune_prepared_cache_locked, remove_pending_prepared_subscriber_locked,
    remove_prepared_ticket_locked, remove_prewarm_frontier_locked,
};
use super::visibility::{
    ordered_session_recipient, refresh_loaded_chunk_for_session_locked,
    refresh_unloaded_chunk_for_session_locked, refresh_visibility_locked, visibility_dispatches,
};
use super::{
    SessionId, SessionRegistry, add_loaded_chunk_reference_locked,
    remove_loaded_chunk_reference_locked, remove_ticket,
};

impl SessionRegistry {
    pub(in crate::play) fn block_entity_data_dispatches(
        &self,
        position: BlockPos,
        except: Option<SessionId>,
        block_entity_type: i32,
        nbt: Tag,
    ) -> Vec<VisibilityDispatch> {
        let chunk = (position.x.div_euclid(16), position.z.div_euclid(16));
        let recipients = {
            let inner = self.lock_inner("block entity data dispatches");
            inner
                .sessions
                .iter()
                .filter(|&(&id, session)| except != Some(id) && session.loaded.contains(&chunk))
                .map(|(&id, session)| {
                    SessionRecipient::unordered(
                        id,
                        session.tx.clone(),
                        Arc::clone(&session.pressure),
                    )
                })
                .collect::<Vec<_>>()
        };
        visibility_dispatches(recipients, || OutboundCommand::BlockEntityData {
            position,
            block_entity_type,
            nbt: nbt.clone(),
        })
    }

    pub(in crate::play) fn replace_view(
        &self,
        id: SessionId,
        center: (i32, i32),
        view_distance: i32,
        desired: HashSet<(i32, i32)>,
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_inner("replace chunk view");
        let (released, acquired, old_frontier, new_frontier, desired_len) = {
            let Some(session) = inner.sessions.get_mut(&id) else {
                return Vec::new();
            };
            let old = std::mem::replace(&mut session.desired, desired);
            let old_frontier = (session.center, session.view_distance, session.pose.yaw);
            session.center = center;
            session.view_distance = view_distance;
            (
                old.difference(&session.desired)
                    .map(|chunk| (*chunk, !session.loaded.contains(chunk)))
                    .collect::<Vec<_>>(),
                session
                    .desired
                    .difference(&old)
                    .map(|chunk| (*chunk, !session.loaded.contains(chunk)))
                    .collect::<Vec<_>>(),
                old_frontier,
                (session.center, session.view_distance, session.pose.yaw),
                session.desired.len(),
            )
        };

        let released_any = !released.is_empty();
        for &(chunk, _) in &released {
            remove_ticket(&mut inner.tickets, chunk, id);
        }
        for &(chunk, _) in &acquired {
            inner.tickets.entry(chunk).or_default().insert(id);
        }
        {
            let mut cache = self.lock_prepared_cache("replace prepared chunk demand");
            for &(chunk, pending) in &released {
                remove_prepared_ticket_locked(&mut cache, chunk, pending);
            }
            for &(chunk, pending) in &acquired {
                add_prepared_ticket_locked(&mut cache, chunk, pending);
            }
            remove_prewarm_frontier_locked(
                &mut cache,
                old_frontier.0,
                old_frontier.1,
                old_frontier.2,
            );
            add_prewarm_frontier_locked(&mut cache, new_frontier.0, new_frontier.1, new_frontier.2);
            prune_prepared_cache_locked(&mut cache);
            self.publish_prepared_cache(&cache);
        }
        let dispatches = refresh_visibility_locked(&mut inner);
        debug!(
            session_id = id,
            center_cx = center.0,
            center_cz = center.1,
            view_distance,
            desired = desired_len,
            global_tickets = inner.tickets.len(),
            shared_tickets = inner.tickets.values().filter(|s| s.len() > 1).count(),
            "play session view tickets replaced"
        );
        drop(inner);
        if released_any {
            self.mark_prepared_changed();
        }
        dispatches
    }

    pub(in crate::play) fn mark_loaded(
        &self,
        id: SessionId,
        chunk: (i32, i32),
    ) -> Vec<VisibilityDispatch> {
        let (dispatches, evicted) = {
            let mut inner = self.lock_inner("mark chunk loaded");
            let inserted = inner
                .sessions
                .get_mut(&id)
                .is_some_and(|session| session.loaded.insert(chunk));
            let was_pending = inserted
                && inner
                    .sessions
                    .get(&id)
                    .is_some_and(|session| session.desired.contains(&chunk));
            if inserted {
                add_loaded_chunk_reference_locked(&mut inner, chunk);
            }
            let dispatches = if inserted {
                refresh_loaded_chunk_for_session_locked(&mut inner, id, chunk)
            } else {
                Vec::new()
            };
            let evicted = if inserted {
                let mut cache = self.lock_prepared_cache("mark prepared chunk loaded");
                if was_pending {
                    remove_pending_prepared_subscriber_locked(&mut cache, chunk);
                }
                let evicted = evict_delivered_prepared_chunk_locked(&mut cache, chunk);
                self.publish_prepared_cache(&cache);
                evicted
            } else {
                false
            };
            (dispatches, evicted)
        };
        if evicted {
            self.mark_prepared_changed();
        }
        dispatches
    }

    pub(in crate::play) fn mark_loaded_if_prepared_revision_current(
        &self,
        id: SessionId,
        chunk: (i32, i32),
        revision: u64,
    ) -> Option<Vec<VisibilityDispatch>> {
        let (dispatches, evicted) = {
            let mut inner = self.lock_inner("mark current prepared chunk loaded");
            let mut cache = self.lock_prepared_cache("check current prepared chunk revision");
            if cache.prepared_revisions.get(&chunk).copied().unwrap_or(0) != revision {
                return None;
            }
            let inserted = inner
                .sessions
                .get_mut(&id)
                .is_some_and(|session| session.loaded.insert(chunk));
            let was_pending = inserted
                && inner
                    .sessions
                    .get(&id)
                    .is_some_and(|session| session.desired.contains(&chunk));
            if inserted {
                add_loaded_chunk_reference_locked(&mut inner, chunk);
            }
            let dispatches = if inserted {
                refresh_loaded_chunk_for_session_locked(&mut inner, id, chunk)
            } else {
                Vec::new()
            };
            if was_pending {
                remove_pending_prepared_subscriber_locked(&mut cache, chunk);
            }
            let evicted = inserted && evict_delivered_prepared_chunk_locked(&mut cache, chunk);
            self.publish_prepared_cache(&cache);
            (dispatches, evicted)
        };
        if evicted {
            self.mark_prepared_changed();
        }
        Some(dispatches)
    }

    pub(in crate::play) fn mark_unloaded(
        &self,
        id: SessionId,
        chunks: &[(i32, i32)],
    ) -> Vec<VisibilityDispatch> {
        let mut inner = self.lock_inner("mark chunks unloaded");
        let mut dispatches = Vec::new();
        if let Some(session) = inner.sessions.get_mut(&id) {
            let mut removed = Vec::new();
            for chunk in chunks {
                if session.loaded.remove(chunk) {
                    removed.push(*chunk);
                }
            }
            for &chunk in &removed {
                remove_loaded_chunk_reference_locked(&mut inner, chunk);
                dispatches.extend(refresh_unloaded_chunk_for_session_locked(
                    &mut inner, id, chunk,
                ));
            }
            if !removed.is_empty() {
                let mut cache = self.lock_prepared_cache("mark prepared chunks unloaded");
                let desired = inner
                    .sessions
                    .get(&id)
                    .expect("session remains present under its session lock");
                for chunk in removed {
                    if desired.desired.contains(&chunk) && !desired.loaded.contains(&chunk) {
                        add_pending_prepared_subscriber_locked(&mut cache, chunk);
                    }
                }
                self.publish_prepared_cache(&cache);
            }
            dispatches
        } else {
            Vec::new()
        }
    }

    pub(in crate::play) fn loaded_recipients_for_chunks(
        &self,
        chunks: &HashSet<(i32, i32)>,
        except: Option<SessionId>,
    ) -> Vec<SessionRecipient> {
        let inner = self.lock_inner("loaded recipients for chunks");
        let mut ids = HashSet::new();
        for chunk in chunks {
            if let Some(subscribers) = inner.tickets.get(chunk) {
                ids.extend(subscribers.iter().copied().filter(|id| Some(*id) != except));
            }
        }
        ids.into_iter()
            .filter_map(|id| {
                let session = inner.sessions.get(&id)?;
                if !chunks.iter().any(|chunk| session.loaded.contains(chunk)) {
                    return None;
                }
                Some(SessionRecipient::unordered(
                    id,
                    session.tx.clone(),
                    Arc::clone(&session.pressure),
                ))
            })
            .collect()
    }

    pub(in crate::play) fn ordered_loaded_recipients_for_chunks(
        &self,
        chunks: &HashSet<(i32, i32)>,
        except: Option<SessionId>,
    ) -> Vec<SessionRecipient> {
        let inner = self.lock_inner("ordered loaded recipients for chunks");
        let mut ids = HashSet::new();
        for chunk in chunks {
            if let Some(subscribers) = inner.tickets.get(chunk) {
                ids.extend(subscribers.iter().copied().filter(|id| Some(*id) != except));
            }
        }
        ids.into_iter()
            .filter_map(|id| {
                let session = inner.sessions.get(&id)?;
                if !chunks.iter().any(|chunk| session.loaded.contains(chunk)) {
                    return None;
                }
                Some(ordered_session_recipient(id, session))
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn ticketed_chunks_sorted(&self) -> Vec<(i32, i32)> {
        let inner = self.lock_inner("ticketed chunks sorted");
        let mut chunks: Vec<_> = inner.tickets.keys().copied().collect();
        chunks.sort_unstable_by_key(|&(cx, cz)| (cz, cx));
        chunks
    }

    pub(crate) fn loaded_chunks_sorted(&self) -> Vec<(i32, i32)> {
        let inner = self.lock_inner("loaded chunks sorted");
        let mut chunks: Vec<_> = inner.loaded_chunk_refcounts.keys().copied().collect();
        chunks.sort_unstable_by_key(|&(cx, cz)| (cz, cx));
        chunks
    }
}
