use super::container_state::CONTAINER_REGISTRY_SHARDS;
use super::outbound::{OrderedDispatchState, OutboundCommand, VisibilityDispatch};
use super::prepared_chunks::{
    add_prepared_ticket_locked, add_prewarm_frontier_locked, prune_prepared_cache_locked,
    release_prepared_claims_for_session_locked, remove_prepared_ticket_locked,
    remove_prewarm_frontier_locked,
};
use super::visibility::{
    refresh_visibility_locked, remove_player_visibility_locked, session_snapshot,
    visibility_dispatches,
};
use super::{
    DisconnectedPlayerPersistence, PlaySession, PublishedCombatTarget, PublishedEntityVisibility,
    SessionAdmissionError, SessionId, SessionPublicationEpoch, SessionRegistration,
    SessionRegistry, remove_loaded_chunk_reference_locked, remove_ticket,
};
#[cfg(test)]
use crate::login::LoggedInProfile;
#[cfg(test)]
use crate::play::PlayerPose;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::Ordering;
#[cfg(test)]
use tokio::sync::mpsc;
use tracing::debug;

impl SessionRegistry {
    pub(crate) fn subscribe_active_sessions(&self) -> tokio::sync::watch::Receiver<usize> {
        self.active_session_sender.subscribe()
    }

    #[cfg(test)]
    pub(in crate::play) fn register(
        &self,
        profile: &LoggedInProfile,
        center: (i32, i32),
        view_distance: i32,
        desired: HashSet<(i32, i32)>,
        tx: mpsc::Sender<OutboundCommand>,
        pose: PlayerPose,
    ) -> (SessionId, Vec<VisibilityDispatch>) {
        self.try_register(SessionRegistration {
            profile,
            properties: &[],
            center,
            view_distance,
            desired,
            tx,
            pose,
            max_sessions: usize::MAX,
            script_operator: false,
            dimension: "minecraft:overworld",
            loader_session: None,
        })
        .expect("unbounded session registration should not fail")
    }

    #[cfg(test)]
    pub(crate) fn register_loaded_for_server_test(
        &self,
        name: &str,
        chunk: (i32, i32),
    ) -> SessionId {
        let profile = LoggedInProfile {
            uuid: uuid::Uuid::from_u128(name.bytes().map(u128::from).sum()),
            name: name.to_string(),
        };
        let (tx, _rx) = mpsc::channel(16);
        let (id, _) = self.register(
            &profile,
            chunk,
            0,
            HashSet::from([chunk]),
            tx,
            PlayerPose::new(
                f64::from(chunk.0 * mc_world::SECTION_DIM as i32) + 0.5,
                64.0,
                f64::from(chunk.1 * mc_world::SECTION_DIM as i32) + 0.5,
            ),
        );
        let _ = self.mark_loaded(id, chunk);
        id
    }

    pub(in crate::play) fn try_register(
        &self,
        registration: SessionRegistration<'_>,
    ) -> Result<(SessionId, Vec<VisibilityDispatch>), SessionAdmissionError> {
        let mut inner = self.lock_inner("register play session");
        if inner.sessions.len() >= registration.max_sessions {
            return Err(SessionAdmissionError::ServerFull {
                active: inner.sessions.len(),
                max: registration.max_sessions,
            });
        }
        if let Some((&existing_session, _)) = inner.sessions.iter().find(|(_, session)| {
            session.uuid == registration.profile.uuid
                || session
                    .name
                    .eq_ignore_ascii_case(&registration.profile.name)
        }) {
            return Err(SessionAdmissionError::DuplicateProfile { existing_session });
        }
        inner.next_id = inner.next_id.wrapping_add(1).max(1);
        let id = inner.next_id;
        let entity_id = i32::try_from(id).unwrap_or(i32::MAX);
        for &chunk in &registration.desired {
            inner.tickets.entry(chunk).or_default().insert(id);
        }
        let pressure = Arc::clone(&self.outbound_pressure);
        let ordered_dispatch = Arc::new(OrderedDispatchState::default());
        let publication_epoch = Arc::new(SessionPublicationEpoch::default());
        inner.sessions.insert(
            id,
            PlaySession {
                name: registration.profile.name.clone(),
                uuid: registration.profile.uuid,
                properties: registration.properties.to_vec(),
                entity_id,
                pose: registration.pose,
                center: registration.center,
                view_distance: registration.view_distance,
                desired: registration.desired,
                loaded: HashSet::new(),
                visible_players: HashSet::new(),
                visible_entities: PublishedEntityVisibility::new(Arc::clone(&publication_epoch)),
                combat_target: PublishedCombatTarget::new(registration.pose, publication_epoch),
                tx: registration.tx,
                pressure,
                ordered_dispatch,
                script_inventory_transaction_gate: Arc::new(
                    super::script_inventory_transaction_endpoint::ScriptInventoryTransactionGate::new(
                    ),
                ),
                script_operator: registration.script_operator,
                dimension: registration.dimension.to_owned(),
                loader_session: registration.loader_session,
            },
        );
        {
            let mut cache = self.lock_prepared_cache("register prepared chunk demand");
            let session = inner
                .sessions
                .get(&id)
                .expect("registered session remains present under its session lock");
            for &chunk in &session.desired {
                add_prepared_ticket_locked(&mut cache, chunk, true);
            }
            add_prewarm_frontier_locked(
                &mut cache,
                session.center,
                session.view_distance,
                session.pose.yaw,
            );
            self.publish_prepared_cache(&cache);
        }
        let dispatches = refresh_visibility_locked(&mut inner);
        self.publish_movement_recipient_index(&inner);
        let became_no_live_sessions = self.publish_live_session_count(&inner);
        debug!(
            session_id = id,
            entity_id,
            player = %registration.profile.name,
            center_cx = registration.center.0,
            center_cz = registration.center.1,
            view_distance = registration.view_distance,
            sessions = inner.sessions.len(),
            tickets = inner.tickets.len(),
            "play session registered"
        );
        self.active_session_sender
            .send_replace(inner.sessions.len());
        drop(inner);
        if became_no_live_sessions {
            self.reconcile_hostile_targets_after_live_session_change();
        }
        Ok((id, dispatches))
    }

    pub(crate) fn active_session_count(&self) -> usize {
        let inner = self.lock_inner("active session count");
        inner.sessions.len()
    }

    pub(crate) fn published_active_session_count(&self) -> usize {
        *self.active_session_sender.borrow()
    }

    pub(crate) fn session_empty_generation(&self) -> u64 {
        self.session_empty_generation.load(Ordering::Acquire)
    }

    pub(crate) async fn wait_for_session_empty(&self, observed: u64) {
        loop {
            let changed = self.session_became_empty.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if self.session_empty_generation() != observed {
                return;
            }
            changed.await;
        }
    }

    fn mark_session_empty(&self) {
        self.session_empty_generation
            .fetch_add(1, Ordering::Release);
        self.session_became_empty.notify_waiters();
    }

    #[cfg(test)]
    pub(crate) fn mark_session_empty_for_test(&self) {
        self.mark_session_empty();
    }

    pub(in crate::play) fn is_active_session(&self, id: SessionId) -> bool {
        let inner = self.lock_inner("validate simulation session fence");
        inner.sessions.contains_key(&id)
    }

    pub(in crate::play) fn has_later_session_at_center(
        &self,
        id: SessionId,
        center: (i32, i32),
    ) -> bool {
        let inner = self.lock_inner("later session at center");
        inner
            .sessions
            .iter()
            .any(|(&other_id, session)| other_id > id && session.center == center)
    }

    pub(in crate::play) fn session_is_at_center(&self, id: SessionId, center: (i32, i32)) -> bool {
        let inner = self.lock_inner("session center check");
        inner
            .sessions
            .get(&id)
            .is_some_and(|session| session.center == center)
    }

    pub(in crate::play) fn session_registration_epoch(&self) -> SessionId {
        let inner = self.lock_inner("session registration epoch");
        inner.next_id
    }

    #[cfg(test)]
    pub(in crate::play) fn unregister(&self, id: SessionId) -> Vec<VisibilityDispatch> {
        self.unregister_inner(id, false)
    }

    pub(in crate::play) fn unregister_preserving_player_state(
        &self,
        id: SessionId,
    ) -> Vec<VisibilityDispatch> {
        self.unregister_inner(id, true)
    }

    fn unregister_inner(
        &self,
        id: SessionId,
        preserve_player_state: bool,
    ) -> Vec<VisibilityDispatch> {
        let script_inventory_transaction_gate = {
            let inner = self.lock_inner("capture unregister script transaction gate");
            let Some(session) = inner.sessions.get(&id) else {
                return Vec::new();
            };
            Arc::clone(&session.script_inventory_transaction_gate)
        };
        script_inventory_transaction_gate.close(id);
        let (
            snapshot,
            recipients,
            completed_sleep,
            player_save_requested,
            became_empty,
            became_no_live_sessions,
        ) = {
            let mut inner = self.lock_inner("unregister play session");
            let Some(session) = inner.sessions.remove(&id) else {
                return Vec::new();
            };
            session.combat_target.close(session.pose);
            let dropped = session.ordered_dispatch.close();
            session.pressure.record_reliable_command_drops(dropped);
            inner.sleeping_sessions.remove(&id);
            inner.spectator_sessions.remove(&id);
            inner.dead_sessions.remove(&id);
            inner.client_unloaded_sessions.remove(&id);
            inner.player_hurt_resistance.remove(&id);
            inner.active_shields.remove(&id);
            inner.shield_disabled_until.remove(&id);
            for &chunk in &session.loaded {
                remove_loaded_chunk_reference_locked(&mut inner, chunk);
            }
            for shard in 0..CONTAINER_REGISTRY_SHARDS {
                let mut containers = self.lock_container_shard(shard, "unregister play session");
                for viewers in containers.furnace_viewers.values_mut() {
                    viewers.remove(&id);
                }
                for viewers in containers.chest_viewers.values_mut() {
                    viewers.remove(&id);
                }
                containers
                    .furnace_viewers
                    .retain(|_, viewers| !viewers.is_empty());
                let active_furnace_positions = containers
                    .furnace_viewers
                    .keys()
                    .copied()
                    .collect::<HashSet<_>>();
                containers
                    .furnace_state_ids
                    .retain(|position, _| active_furnace_positions.contains(position));
                containers
                    .chest_viewers
                    .retain(|_, viewers| !viewers.is_empty());
                let active_chest_positions = containers
                    .chest_viewers
                    .keys()
                    .copied()
                    .collect::<HashSet<_>>();
                containers
                    .chest_state_ids
                    .retain(|position, _| active_chest_positions.contains(position));
            }
            let player_state = inner.player_persistence.remove(&id);
            let player_save_requested = if preserve_player_state {
                if let Some(state) = player_state {
                    let generation = inner
                        .next_disconnected_player_generation
                        .wrapping_add(1)
                        .max(1);
                    inner.next_disconnected_player_generation = generation;
                    inner.disconnected_player_persistence.insert(
                        session.uuid,
                        DisconnectedPlayerPersistence { generation, state },
                    );
                    true
                } else {
                    false
                }
            } else {
                false
            };
            let snapshot = session_snapshot(id, &session);
            let recipients = remove_player_visibility_locked(&mut inner, id);
            let desired_len = session.desired.len();
            let loaded_len = session.loaded.len();
            for &chunk in &session.desired {
                remove_ticket(&mut inner.tickets, chunk, id);
            }
            let released_prepare_claims = {
                let mut cache = self.lock_prepared_cache("unregister prepared chunk demand");
                for &chunk in &session.desired {
                    remove_prepared_ticket_locked(
                        &mut cache,
                        chunk,
                        !session.loaded.contains(&chunk),
                    );
                }
                let released_prepare_claims =
                    release_prepared_claims_for_session_locked(&mut cache, id);
                remove_prewarm_frontier_locked(
                    &mut cache,
                    session.center,
                    session.view_distance,
                    session.pose.yaw,
                );
                prune_prepared_cache_locked(&mut cache);
                self.publish_prepared_cache(&cache);
                released_prepare_claims
            };
            debug!(
                session_id = id,
                player = %session.name,
                desired = desired_len,
                loaded = loaded_len,
                sessions = inner.sessions.len(),
                tickets = inner.tickets.len(),
                released_prepare_claims,
                player_save_requested,
                "play session unregistered"
            );
            let active_sessions = inner.sessions.len();
            let completed_sleep = self.resolve_sleep_transition_locked(&mut inner);
            self.active_session_sender.send_replace(active_sessions);
            self.publish_movement_recipient_index(&inner);
            let became_no_live_sessions = self.publish_live_session_count(&inner);
            (
                snapshot,
                recipients,
                completed_sleep,
                player_save_requested,
                active_sessions == 0,
                became_no_live_sessions,
            )
        };
        if became_no_live_sessions {
            self.reconcile_hostile_targets_after_live_session_change();
        }
        if became_empty {
            self.mark_session_empty();
        }
        if player_save_requested {
            self.mark_player_save_requested();
        }
        self.mark_prepared_changed();
        let mut dispatches = visibility_dispatches(recipients, || {
            OutboundCommand::DespawnPlayer(snapshot.clone())
        });
        dispatches.extend(self.sleep_transition_dispatches(completed_sleep));
        dispatches
    }
}
