use super::{ServerEntitySnapshot, SessionEntityGuards, SessionId};
use crate::play::persistence::PlayerPersistedState;
use mc_entity::Vec3;
use mc_protocol::packets::play::GameMode;
use mc_script::{
    ScriptEntityId, ScriptEntityKillSource, ScriptEvent, ScriptGameMode, ScriptPlayerContext,
    ScriptPlayerId,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::sync::{mpsc, watch};
use tracing::warn;

pub(crate) const SCRIPT_COMMIT_EVENT_OUTBOX_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScriptCommitDelivery {
    Required,
    #[allow(
        dead_code,
        reason = "explicit delivery policy; production producers are currently required"
    )]
    BestEffort,
}

#[derive(Debug)]
pub(crate) struct ScriptCommitEventEnvelope {
    pub(crate) delivery: ScriptCommitDelivery,
    pub(crate) event: ScriptEvent,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ScriptCommitEventOutboxSnapshot {
    pub(crate) capacity: usize,
    pub(crate) depth: usize,
    pub(crate) max_depth: usize,
    pub(crate) enqueued: u64,
    pub(crate) dequeued: u64,
    pub(crate) required_overflow: u64,
    pub(crate) required_closed: u64,
    pub(crate) best_effort_dropped: u64,
    pub(crate) best_effort_sink_dropped: u64,
    pub(crate) abandoned_on_receiver_drop: u64,
    pub(crate) required_abandoned_on_receiver_drop: u64,
}

#[derive(Debug)]
pub(super) struct ScriptCommitEventMetrics {
    capacity: usize,
    depth: AtomicUsize,
    max_depth: AtomicUsize,
    enqueued: AtomicU64,
    dequeued: AtomicU64,
    required_overflow: AtomicU64,
    required_closed: AtomicU64,
    best_effort_dropped: AtomicU64,
    best_effort_sink_dropped: AtomicU64,
    abandoned_on_receiver_drop: AtomicU64,
    required_abandoned_on_receiver_drop: AtomicU64,
}

impl Default for ScriptCommitEventMetrics {
    fn default() -> Self {
        Self::new(SCRIPT_COMMIT_EVENT_OUTBOX_CAPACITY)
    }
}

impl ScriptCommitEventMetrics {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            depth: AtomicUsize::new(0),
            max_depth: AtomicUsize::new(0),
            enqueued: AtomicU64::new(0),
            dequeued: AtomicU64::new(0),
            required_overflow: AtomicU64::new(0),
            required_closed: AtomicU64::new(0),
            best_effort_dropped: AtomicU64::new(0),
            best_effort_sink_dropped: AtomicU64::new(0),
            abandoned_on_receiver_drop: AtomicU64::new(0),
            required_abandoned_on_receiver_drop: AtomicU64::new(0),
        }
    }
}

impl ScriptCommitEventMetrics {
    fn record_depth(&self, depth: usize) {
        let mut observed = self.max_depth.load(Ordering::Relaxed);
        while depth > observed {
            match self.max_depth.compare_exchange_weak(
                observed,
                depth,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => observed = actual,
            }
        }
    }

    pub(super) fn snapshot(&self) -> ScriptCommitEventOutboxSnapshot {
        ScriptCommitEventOutboxSnapshot {
            capacity: self.capacity,
            depth: self.depth.load(Ordering::Relaxed),
            max_depth: self.max_depth.load(Ordering::Relaxed),
            enqueued: self.enqueued.load(Ordering::Relaxed),
            dequeued: self.dequeued.load(Ordering::Relaxed),
            required_overflow: self.required_overflow.load(Ordering::Relaxed),
            required_closed: self.required_closed.load(Ordering::Relaxed),
            best_effort_dropped: self.best_effort_dropped.load(Ordering::Relaxed),
            best_effort_sink_dropped: self.best_effort_sink_dropped.load(Ordering::Relaxed),
            abandoned_on_receiver_drop: self.abandoned_on_receiver_drop.load(Ordering::Relaxed),
            required_abandoned_on_receiver_drop: self
                .required_abandoned_on_receiver_drop
                .load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScriptCommitEnqueueOutcome {
    Enqueued,
    BestEffortDropped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScriptCommitEnqueueError {
    RequiredOverflow,
    RequiredClosed,
}

#[derive(Debug, Clone)]
pub(super) struct ScriptCommitEventOutbox {
    sender: mpsc::Sender<ScriptCommitEventEnvelope>,
    metrics: Arc<ScriptCommitEventMetrics>,
    required_failure: watch::Sender<bool>,
}

impl ScriptCommitEventOutbox {
    pub(super) fn new(
        sender: mpsc::Sender<ScriptCommitEventEnvelope>,
        metrics: Arc<ScriptCommitEventMetrics>,
        required_failure: watch::Sender<bool>,
    ) -> Self {
        Self {
            sender,
            metrics,
            required_failure,
        }
    }

    pub(super) fn try_enqueue(
        &self,
        delivery: ScriptCommitDelivery,
        event: ScriptEvent,
    ) -> Result<ScriptCommitEnqueueOutcome, ScriptCommitEnqueueError> {
        match self.sender.try_reserve() {
            Ok(permit) => {
                self.metrics.enqueued.fetch_add(1, Ordering::Relaxed);
                let depth = self.metrics.depth.fetch_add(1, Ordering::Relaxed) + 1;
                self.metrics.record_depth(depth);
                permit.send(ScriptCommitEventEnvelope { delivery, event });
                Ok(ScriptCommitEnqueueOutcome::Enqueued)
            }
            Err(mpsc::error::TrySendError::Full(_)) => match delivery {
                ScriptCommitDelivery::Required => {
                    self.metrics
                        .required_overflow
                        .fetch_add(1, Ordering::Relaxed);
                    self.required_failure.send_replace(true);
                    Err(ScriptCommitEnqueueError::RequiredOverflow)
                }
                ScriptCommitDelivery::BestEffort => {
                    self.metrics
                        .best_effort_dropped
                        .fetch_add(1, Ordering::Relaxed);
                    Ok(ScriptCommitEnqueueOutcome::BestEffortDropped)
                }
            },
            Err(mpsc::error::TrySendError::Closed(_)) => match delivery {
                ScriptCommitDelivery::Required => {
                    self.metrics.required_closed.fetch_add(1, Ordering::Relaxed);
                    self.required_failure.send_replace(true);
                    Err(ScriptCommitEnqueueError::RequiredClosed)
                }
                ScriptCommitDelivery::BestEffort => {
                    self.metrics
                        .best_effort_dropped
                        .fetch_add(1, Ordering::Relaxed);
                    Ok(ScriptCommitEnqueueOutcome::BestEffortDropped)
                }
            },
        }
    }
}

pub(crate) struct ScriptCommitEventReceiver {
    receiver: mpsc::Receiver<ScriptCommitEventEnvelope>,
    metrics: Arc<ScriptCommitEventMetrics>,
    required_failure: watch::Sender<bool>,
}

impl Drop for ScriptCommitEventReceiver {
    fn drop(&mut self) {
        self.receiver.close();
        let mut abandoned = 0_usize;
        let mut required_abandoned = 0_usize;
        while let Ok(envelope) = self.receiver.try_recv() {
            abandoned = abandoned.saturating_add(1);
            if envelope.delivery == ScriptCommitDelivery::Required {
                required_abandoned = required_abandoned.saturating_add(1);
            }
        }
        if abandoned == 0 {
            return;
        }
        self.metrics.depth.fetch_sub(abandoned, Ordering::Relaxed);
        self.metrics.abandoned_on_receiver_drop.fetch_add(
            u64::try_from(abandoned).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        if required_abandoned != 0 {
            self.metrics.required_abandoned_on_receiver_drop.fetch_add(
                u64::try_from(required_abandoned).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
            self.required_failure.send_replace(true);
        }
    }
}

impl ScriptCommitEventReceiver {
    pub(super) fn new(
        receiver: mpsc::Receiver<ScriptCommitEventEnvelope>,
        metrics: Arc<ScriptCommitEventMetrics>,
        required_failure: watch::Sender<bool>,
    ) -> Self {
        Self {
            receiver,
            metrics,
            required_failure,
        }
    }

    pub(crate) async fn recv(&mut self) -> Option<ScriptCommitEventEnvelope> {
        let envelope = self.receiver.recv().await?;
        self.metrics.depth.fetch_sub(1, Ordering::Relaxed);
        self.metrics.dequeued.fetch_add(1, Ordering::Relaxed);
        Some(envelope)
    }

    #[cfg(test)]
    pub(crate) fn try_recv(
        &mut self,
    ) -> Result<ScriptCommitEventEnvelope, mpsc::error::TryRecvError> {
        let envelope = self.receiver.try_recv()?;
        self.metrics.depth.fetch_sub(1, Ordering::Relaxed);
        self.metrics.dequeued.fetch_add(1, Ordering::Relaxed);
        Ok(envelope)
    }

    #[cfg(test)]
    pub(crate) fn try_recv_required(&mut self) -> Result<ScriptEvent, mpsc::error::TryRecvError> {
        let envelope = self.try_recv()?;
        assert_eq!(envelope.delivery, ScriptCommitDelivery::Required);
        Ok(envelope.event)
    }

    pub(crate) fn report_required_failure(&self) {
        self.required_failure.send_replace(true);
    }

    pub(crate) fn record_best_effort_sink_drop(&self) {
        self.metrics
            .best_effort_sink_dropped
            .fetch_add(1, Ordering::Relaxed);
    }
}

pub(super) fn push_player_death_event_locked(
    inner: &SessionEntityGuards<'_>,
    actor_session: SessionId,
    player_state: &PlayerPersistedState,
    position: Vec3,
) {
    let Some(sender) = inner.script_commit_events.as_ref() else {
        return;
    };
    let Some(session) = inner.sessions.get(&actor_session) else {
        warn!(
            session_id = actor_session,
            "committed player death lost its session snapshot"
        );
        return;
    };
    let game_mode = match player_state.game_mode {
        GameMode::Survival => ScriptGameMode::Survival,
        GameMode::Adventure => ScriptGameMode::Adventure,
        game_mode => {
            warn!(
                session_id = actor_session,
                ?game_mode,
                "committed player death has unsupported script game mode"
            );
            return;
        }
    };
    let context = match ScriptPlayerContext::try_new(
        session.uuid.to_string(),
        &session.name,
        session.script_operator,
        position.x,
        position.y,
        position.z,
    ) {
        Ok(context) => context,
        Err(error) => {
            warn!(
                session_id = actor_session,
                ?error,
                "committed player death context is invalid"
            );
            return;
        }
    };
    let event = match ScriptEvent::try_player_died_with_context(
        ScriptPlayerId::new(actor_session),
        context,
        &session.dimension,
        game_mode,
    ) {
        Ok(event) => event,
        Err(error) => {
            warn!(
                session_id = actor_session,
                ?error,
                "committed player death script event is invalid"
            );
            return;
        }
    };
    if let Err(error) = sender.try_enqueue(ScriptCommitDelivery::Required, event) {
        warn!(
            session_id = actor_session,
            ?error,
            "required committed player-death script event could not enter bounded outbox"
        );
    }
}

pub(super) fn push_player_entity_killed_event_locked(
    inner: &SessionEntityGuards<'_>,
    actor_session: SessionId,
    game_mode: GameMode,
    player_pose: crate::play::PlayerPose,
    entity: &ServerEntitySnapshot,
) {
    let Some(sender) = inner.script_commit_events.as_ref() else {
        return;
    };
    let Some(session) = inner.sessions.get(&actor_session) else {
        warn!(
            session_id = actor_session,
            entity_id = entity.id.0,
            "committed entity kill lost its player session snapshot"
        );
        return;
    };
    let game_mode = match game_mode {
        GameMode::Survival => ScriptGameMode::Survival,
        GameMode::Creative => ScriptGameMode::Creative,
        GameMode::Adventure => ScriptGameMode::Adventure,
        game_mode => {
            warn!(
                session_id = actor_session,
                ?game_mode,
                "committed entity kill has unsupported script game mode"
            );
            return;
        }
    };
    let Ok(entity_id) = u64::try_from(entity.id.0) else {
        warn!(
            session_id = actor_session,
            entity_id = entity.id.0,
            "committed entity kill has invalid script entity id"
        );
        return;
    };
    let context = match ScriptPlayerContext::try_new(
        session.uuid.to_string(),
        &session.name,
        session.script_operator,
        player_pose.x,
        player_pose.y,
        player_pose.z,
    ) {
        Ok(context) => context,
        Err(error) => {
            warn!(
                session_id = actor_session,
                ?error,
                "committed entity kill player context is invalid"
            );
            return;
        }
    };
    let event = match ScriptEvent::try_player_entity_killed_with_context(
        ScriptPlayerId::new(actor_session),
        context,
        &session.dimension,
        ScriptEntityId::new(entity_id),
        &entity.type_name,
        ScriptEntityKillSource::Melee,
        game_mode,
    ) {
        Ok(event) => event,
        Err(error) => {
            warn!(
                session_id = actor_session,
                entity_id = entity.id.0,
                ?error,
                "committed entity kill script event is invalid"
            );
            return;
        }
    };
    if let Err(error) = sender.try_enqueue(ScriptCommitDelivery::Required, event) {
        warn!(
            session_id = actor_session,
            entity_id = entity.id.0,
            ?error,
            "required committed entity-kill script event could not enter bounded outbox"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outbox(
        capacity: usize,
    ) -> (
        ScriptCommitEventOutbox,
        ScriptCommitEventReceiver,
        watch::Receiver<bool>,
        Arc<ScriptCommitEventMetrics>,
    ) {
        let (sender, receiver) = mpsc::channel(capacity);
        let metrics = Arc::new(ScriptCommitEventMetrics::new(capacity));
        let (failure, failure_rx) = watch::channel(false);
        (
            ScriptCommitEventOutbox::new(sender, Arc::clone(&metrics), failure.clone()),
            ScriptCommitEventReceiver::new(receiver, Arc::clone(&metrics), failure),
            failure_rx,
            metrics,
        )
    }

    #[test]
    fn required_overflow_is_fatal_and_bounded() {
        let (outbox, _receiver, mut failure, metrics) = outbox(1);
        assert_eq!(
            outbox.try_enqueue(
                ScriptCommitDelivery::Required,
                ScriptEvent::server_started(),
            ),
            Ok(ScriptCommitEnqueueOutcome::Enqueued)
        );
        assert_eq!(
            outbox.try_enqueue(
                ScriptCommitDelivery::Required,
                ScriptEvent::server_stopping("overflow"),
            ),
            Err(ScriptCommitEnqueueError::RequiredOverflow)
        );
        assert!(*failure.borrow_and_update());
        assert_eq!(
            metrics.snapshot(),
            ScriptCommitEventOutboxSnapshot {
                capacity: 1,
                depth: 1,
                max_depth: 1,
                enqueued: 1,
                required_overflow: 1,
                ..ScriptCommitEventOutboxSnapshot::default()
            }
        );
    }

    #[test]
    fn best_effort_overflow_drops_without_fatal_failure() {
        let (outbox, _receiver, mut failure, metrics) = outbox(1);
        outbox
            .try_enqueue(
                ScriptCommitDelivery::Required,
                ScriptEvent::server_started(),
            )
            .unwrap();
        assert_eq!(
            outbox.try_enqueue(
                ScriptCommitDelivery::BestEffort,
                ScriptEvent::server_tick(1),
            ),
            Ok(ScriptCommitEnqueueOutcome::BestEffortDropped)
        );
        assert!(!*failure.borrow_and_update());
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.depth, 1);
        assert_eq!(snapshot.best_effort_dropped, 1);
        assert_eq!(snapshot.required_overflow, 0);
    }

    #[test]
    fn receiver_updates_depth_and_delivery_metrics() {
        let (outbox, mut receiver, _failure, metrics) = outbox(2);
        outbox
            .try_enqueue(
                ScriptCommitDelivery::Required,
                ScriptEvent::server_started(),
            )
            .unwrap();
        outbox
            .try_enqueue(
                ScriptCommitDelivery::BestEffort,
                ScriptEvent::server_tick(2),
            )
            .unwrap();

        let required = receiver.try_recv().unwrap();
        assert_eq!(required.delivery, ScriptCommitDelivery::Required);
        let best_effort = receiver.try_recv().unwrap();
        assert_eq!(best_effort.delivery, ScriptCommitDelivery::BestEffort);
        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.depth, 0);
        assert_eq!(snapshot.max_depth, 2);
        assert_eq!(snapshot.enqueued, 2);
        assert_eq!(snapshot.dequeued, 2);
    }

    #[test]
    fn receiver_drop_accounts_for_bounded_backlog_and_fails_required_delivery() {
        let (outbox, receiver, mut failure, metrics) = outbox(2);
        outbox
            .try_enqueue(
                ScriptCommitDelivery::Required,
                ScriptEvent::server_started(),
            )
            .unwrap();
        outbox
            .try_enqueue(
                ScriptCommitDelivery::BestEffort,
                ScriptEvent::server_tick(2),
            )
            .unwrap();
        drop(receiver);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.depth, 0);
        assert_eq!(snapshot.abandoned_on_receiver_drop, 2);
        assert_eq!(snapshot.required_abandoned_on_receiver_drop, 1);
        assert!(*failure.borrow_and_update());
    }

    #[test]
    fn receiver_drop_with_only_best_effort_backlog_is_not_fatal() {
        let (outbox, receiver, mut failure, metrics) = outbox(2);
        outbox
            .try_enqueue(
                ScriptCommitDelivery::BestEffort,
                ScriptEvent::server_tick(1),
            )
            .unwrap();
        drop(receiver);

        let snapshot = metrics.snapshot();
        assert_eq!(snapshot.depth, 0);
        assert_eq!(snapshot.abandoned_on_receiver_drop, 1);
        assert_eq!(snapshot.required_abandoned_on_receiver_drop, 0);
        assert!(!*failure.borrow_and_update());
    }

    #[test]
    fn required_send_after_receiver_drop_is_fatal() {
        let (outbox, receiver, mut failure, metrics) = outbox(1);
        drop(receiver);
        assert_eq!(
            outbox.try_enqueue(
                ScriptCommitDelivery::Required,
                ScriptEvent::server_started(),
            ),
            Err(ScriptCommitEnqueueError::RequiredClosed)
        );
        assert!(*failure.borrow_and_update());
        assert_eq!(metrics.snapshot().required_closed, 1);
    }
}
