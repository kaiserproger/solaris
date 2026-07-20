use std::fmt;
use std::future::Future;
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::{Notify, oneshot};

#[derive(Debug)]
pub(crate) enum DirtyFlushWriteError {
    World(mc_world::storage::WorldError),
    Join(tokio::task::JoinError),
}

impl DirtyFlushWriteError {
    #[must_use]
    pub(crate) fn is_stale_region(&self) -> bool {
        matches!(
            self,
            Self::World(mc_world::storage::WorldError::StaleRegion(_))
        )
    }
}

impl fmt::Display for DirtyFlushWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::World(err) => write!(f, "{err}"),
            Self::Join(err) => write!(f, "{err}"),
        }
    }
}

pub(crate) async fn write_dirty_flush_blocking(
    flush_plan: mc_world::storage::DirtyFlushPlan,
) -> Result<mc_world::storage::DirtyFlushCommit, String> {
    write_dirty_flush_blocking_typed(flush_plan)
        .await
        .map_err(|err| err.to_string())
}

pub(crate) async fn write_dirty_flush_blocking_typed(
    flush_plan: mc_world::storage::DirtyFlushPlan,
) -> Result<mc_world::storage::DirtyFlushCommit, DirtyFlushWriteError> {
    match tokio::task::spawn_blocking(move || flush_plan.write()).await {
        Ok(Ok(commit)) => Ok(commit),
        Ok(Err(err)) => Err(DirtyFlushWriteError::World(err)),
        Err(err) => Err(DirtyFlushWriteError::Join(err)),
    }
}

pub(crate) async fn sync_dirty_flush_install_blocking_typed(
    install: mc_world::storage::DirtyFlushInstall,
) -> Result<mc_world::storage::DirtyFlushSynced, DirtyFlushWriteError> {
    match tokio::task::spawn_blocking(move || install.sync()).await {
        Ok(Ok(synced)) => Ok(synced),
        Ok(Err(err)) => Err(DirtyFlushWriteError::World(err)),
        Err(err) => Err(DirtyFlushWriteError::Join(err)),
    }
}

#[derive(Default)]
struct DirtyFlushState {
    dirty_flush: bool,
    full_checkpoint: bool,
    drain: Option<oneshot::Sender<()>>,
    stopped: bool,
}

#[derive(Default)]
struct DirtyFlushShared {
    state: Mutex<DirtyFlushState>,
    wake: Notify,
}

/// Push-only producer handle for the server-owned save worker.
#[derive(Debug, Clone)]
pub(crate) struct DirtyFlushNotifier {
    shared: Weak<DirtyFlushShared>,
}

impl DirtyFlushNotifier {
    /// Compatibility shorthand for a dirty-only flush request.
    #[cfg(test)]
    pub(crate) fn request(&self) {
        self.request_dirty_flush();
    }

    /// Mark a dirty-only flush pending. Repeated notifications coalesce until
    /// the consumer starts its next flush.
    pub(crate) fn request_dirty_flush(&self) {
        self.request_action(DirtyFlushRequest::DirtyOnly);
    }

    /// Mark a full player/entity/metadata/WAL checkpoint pending.
    pub(crate) fn request_full_checkpoint(&self) {
        self.request_action(DirtyFlushRequest::FullCheckpoint);
    }

    fn request_action(&self, request: DirtyFlushRequest) {
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        let mut state = shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.stopped {
            tracing::warn!("dirty flush worker stopped before producer notification");
            return;
        }
        match request {
            DirtyFlushRequest::DirtyOnly => state.dirty_flush = true,
            DirtyFlushRequest::FullCheckpoint => state.full_checkpoint = true,
        }
        drop(state);
        shared.wake.notify_one();
    }
}

enum DirtyFlushRequest {
    DirtyOnly,
    FullCheckpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DirtyFlushCompletion {
    Complete,
    MoreDirty,
    AwaitingProducer,
    Failed,
}

/// Owns the coalesced dirty/full request queue and its worker lifecycle.
///
/// The worker deliberately owns no world handle. Its caller provides the
/// server-specific flush operation, keeping world access and save policy with
/// the server while producers retain only a push notification handle.
pub(crate) struct DirtyFlushCoordinator {
    shared: Arc<DirtyFlushShared>,
    worker: tokio::task::JoinHandle<()>,
}

#[derive(Debug)]
pub(crate) enum DirtyFlushDrainError {
    CompletionDropped,
    WorkerStoppedBeforeCompletion,
    WorkerJoin(tokio::task::JoinError),
}

impl fmt::Display for DirtyFlushDrainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CompletionDropped => write!(f, "dirty flush drain completion was dropped"),
            Self::WorkerStoppedBeforeCompletion => {
                write!(f, "dirty flush worker stopped before drain completion")
            }
            Self::WorkerJoin(error) => write!(f, "dirty flush worker join failed: {error}"),
        }
    }
}

impl std::error::Error for DirtyFlushDrainError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::WorkerJoin(error) => Some(error),
            Self::CompletionDropped | Self::WorkerStoppedBeforeCompletion => None,
        }
    }
}

#[derive(Debug)]
pub(crate) enum DirtyFlushDrainOutcome {
    Complete,
    Failed(DirtyFlushDrainError),
}

impl DirtyFlushDrainOutcome {
    pub(crate) fn into_result(self) -> Result<(), DirtyFlushDrainError> {
        match self {
            Self::Complete => Ok(()),
            Self::Failed(error) => Err(error),
        }
    }
}

impl DirtyFlushCoordinator {
    #[cfg(test)]
    pub(crate) fn spawn<Flush, FlushFuture>(mut flush: Flush) -> Self
    where
        Flush: FnMut() -> FlushFuture + Send + 'static,
        FlushFuture: Future<Output = ()> + Send + 'static,
    {
        Self::spawn_actions(
            move || {
                let flush = flush();
                async move {
                    flush.await;
                    DirtyFlushCompletion::Complete
                }
            },
            || async {},
        )
    }

    /// Run coalesced dirty-only work independently from full checkpoints.
    /// Both actions share one serial worker so region writes and checkpoints
    /// cannot overlap through this queue, while their trigger policies remain
    /// distinct.
    pub(crate) fn spawn_actions<
        DirtyFlush,
        DirtyFlushFuture,
        FullCheckpoint,
        FullCheckpointFuture,
    >(
        mut dirty_flush: DirtyFlush,
        mut full_checkpoint: FullCheckpoint,
    ) -> Self
    where
        DirtyFlush: FnMut() -> DirtyFlushFuture + Send + 'static,
        DirtyFlushFuture: Future<Output = DirtyFlushCompletion> + Send + 'static,
        FullCheckpoint: FnMut() -> FullCheckpointFuture + Send + 'static,
        FullCheckpointFuture: Future<Output = ()> + Send + 'static,
    {
        let shared = Arc::new(DirtyFlushShared::default());
        let worker_shared = Arc::clone(&shared);
        let worker = tokio::spawn(async move {
            loop {
                worker_shared.wake.notified().await;
                loop {
                    let action = {
                        let mut state = worker_shared
                            .state
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                        if state.full_checkpoint {
                            state.full_checkpoint = false;
                            // A full checkpoint includes dirty chunks, so it
                            // subsumes dirty work published before selection.
                            // Requests published during the checkpoint set the
                            // bit again and run afterward.
                            state.dirty_flush = false;
                            DirtyFlushAction::FullCheckpoint
                        } else if state.dirty_flush {
                            state.dirty_flush = false;
                            DirtyFlushAction::DirtyOnly
                        } else if let Some(completed) = state.drain.take() {
                            state.stopped = true;
                            DirtyFlushAction::Stop(completed)
                        } else {
                            DirtyFlushAction::Wait
                        }
                    };
                    match action {
                        DirtyFlushAction::DirtyOnly => {
                            if dirty_flush().await == DirtyFlushCompletion::MoreDirty {
                                let mut state = worker_shared
                                    .state
                                    .lock()
                                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                                state.dirty_flush = true;
                                drop(state);
                                worker_shared.wake.notify_one();
                            }
                        }
                        DirtyFlushAction::FullCheckpoint => full_checkpoint().await,
                        DirtyFlushAction::Stop(completed) => {
                            let _ = completed.send(());
                            return;
                        }
                        DirtyFlushAction::Wait => break,
                    }
                }
            }
        });
        Self { shared, worker }
    }

    #[must_use]
    pub(crate) fn notifier(&self) -> DirtyFlushNotifier {
        DirtyFlushNotifier {
            shared: Arc::downgrade(&self.shared),
        }
    }

    /// Wait until the consumer has processed every request published before
    /// this drain marker, then stop the worker.
    pub(crate) async fn drain(self) -> DirtyFlushDrainOutcome {
        let (completed, received) = oneshot::channel();
        {
            let mut state = self
                .shared
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            state.drain = Some(completed);
        }
        self.shared.wake.notify_one();
        let mut worker = self.worker;
        tokio::select! {
            biased;
            completion = received => {
                if completion.is_err() {
                    return match worker.await {
                        Ok(()) => DirtyFlushDrainOutcome::Failed(
                            DirtyFlushDrainError::CompletionDropped,
                        ),
                        Err(error) => DirtyFlushDrainOutcome::Failed(
                            DirtyFlushDrainError::WorkerJoin(error),
                        ),
                    };
                }
                match worker.await {
                    Ok(()) => DirtyFlushDrainOutcome::Complete,
                    Err(error) => DirtyFlushDrainOutcome::Failed(
                        DirtyFlushDrainError::WorkerJoin(error),
                    ),
                }
            }
            result = &mut worker => match result {
                Ok(()) => DirtyFlushDrainOutcome::Failed(
                    DirtyFlushDrainError::WorkerStoppedBeforeCompletion,
                ),
                Err(error) => DirtyFlushDrainOutcome::Failed(
                    DirtyFlushDrainError::WorkerJoin(error),
                ),
            },
        }
    }
}

enum DirtyFlushAction {
    DirtyOnly,
    FullCheckpoint,
    Stop(oneshot::Sender<()>),
    Wait,
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::{mpsc, oneshot};

    use super::{
        DirtyFlushCompletion, DirtyFlushCoordinator, DirtyFlushDrainError, DirtyFlushDrainOutcome,
    };

    #[tokio::test]
    async fn awaiting_producer_completion_does_not_self_rearm_and_allows_drain() {
        let dirty_calls = Arc::new(AtomicUsize::new(0));
        let coordinator = DirtyFlushCoordinator::spawn_actions(
            {
                let dirty_calls = Arc::clone(&dirty_calls);
                move || {
                    dirty_calls.fetch_add(1, Ordering::SeqCst);
                    async { DirtyFlushCompletion::AwaitingProducer }
                }
            },
            || async {},
        );

        coordinator.notifier().request_dirty_flush();
        coordinator.drain().await;

        assert_eq!(dirty_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_completion_retries_only_after_a_new_producer_notification() {
        let dirty_calls = Arc::new(AtomicUsize::new(0));
        let (completed, mut completed_rx) = mpsc::channel(2);
        let coordinator = DirtyFlushCoordinator::spawn_actions(
            {
                let dirty_calls = Arc::clone(&dirty_calls);
                move || {
                    let call = dirty_calls.fetch_add(1, Ordering::SeqCst);
                    let completed = completed.clone();
                    async move {
                        completed
                            .send(call)
                            .await
                            .expect("test observes completion");
                        if call == 0 {
                            DirtyFlushCompletion::Failed
                        } else {
                            DirtyFlushCompletion::Complete
                        }
                    }
                }
            },
            || async {},
        );
        let notifier = coordinator.notifier();

        notifier.request_dirty_flush();
        assert_eq!(completed_rx.recv().await, Some(0));
        notifier.request_dirty_flush();
        assert_eq!(completed_rx.recv().await, Some(1));
        coordinator.drain().await;

        assert_eq!(dirty_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn dirty_completion_requeues_tail_until_complete() {
        let dirty_calls = Arc::new(AtomicUsize::new(0));
        let coordinator = DirtyFlushCoordinator::spawn_actions(
            {
                let dirty_calls = Arc::clone(&dirty_calls);
                move || {
                    let call = dirty_calls.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if call == 0 {
                            DirtyFlushCompletion::MoreDirty
                        } else {
                            DirtyFlushCompletion::Complete
                        }
                    }
                }
            },
            || async {},
        );

        coordinator.notifier().request_dirty_flush();
        coordinator.drain().await;

        assert_eq!(dirty_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn full_checkpoint_subsumes_requeued_dirty_tail() {
        let dirty_calls = Arc::new(AtomicUsize::new(0));
        let (dirty_started, mut dirty_started_rx) = mpsc::channel(1);
        let (dirty_release, dirty_release_rx) = oneshot::channel();
        let mut dirty_release_rx = Some(dirty_release_rx);
        let (full_started, mut full_started_rx) = mpsc::channel(1);
        let coordinator = DirtyFlushCoordinator::spawn_actions(
            {
                let dirty_calls = Arc::clone(&dirty_calls);
                move || {
                    let dirty_calls = Arc::clone(&dirty_calls);
                    let dirty_started = dirty_started.clone();
                    let dirty_release = dirty_release_rx
                        .take()
                        .expect("one dirty invocation is expected");
                    async move {
                        dirty_calls.fetch_add(1, Ordering::SeqCst);
                        dirty_started
                            .send(())
                            .await
                            .expect("test observes dirty flush");
                        dirty_release.await.expect("test releases dirty flush");
                        DirtyFlushCompletion::MoreDirty
                    }
                }
            },
            move || {
                let full_started = full_started.clone();
                async move {
                    full_started
                        .send(())
                        .await
                        .expect("test observes full checkpoint");
                }
            },
        );
        let notifier = coordinator.notifier();

        notifier.request_dirty_flush();
        assert_eq!(dirty_started_rx.recv().await, Some(()));
        notifier.request_full_checkpoint();
        let drain = tokio::spawn(coordinator.drain());
        dirty_release
            .send(())
            .expect("dirty flush is waiting for release");
        assert_eq!(full_started_rx.recv().await, Some(()));
        drain.await.expect("drain task joins");

        assert_eq!(dirty_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn continuous_dirty_requests_do_not_run_full_checkpoints_and_drain_tail() {
        let dirty_calls = Arc::new(AtomicUsize::new(0));
        let full_calls = Arc::new(AtomicUsize::new(0));
        let (started, mut started_rx) = mpsc::channel(2);
        let (first_release, first_release_rx) = oneshot::channel();
        let mut first_release_rx = Some(first_release_rx);
        let coordinator = DirtyFlushCoordinator::spawn_actions(
            {
                let dirty_calls = Arc::clone(&dirty_calls);
                move || {
                    let dirty_calls = Arc::clone(&dirty_calls);
                    let started = started.clone();
                    let first_release = first_release_rx.take();
                    async move {
                        let call = dirty_calls.fetch_add(1, Ordering::SeqCst);
                        started.send(call).await.expect("test observes dirty flush");
                        if let Some(release) = first_release {
                            release.await.expect("test releases first dirty flush");
                        }
                        DirtyFlushCompletion::Complete
                    }
                }
            },
            {
                let full_calls = Arc::clone(&full_calls);
                move || {
                    let full_calls = Arc::clone(&full_calls);
                    async move {
                        full_calls.fetch_add(1, Ordering::SeqCst);
                    }
                }
            },
        );
        let notifier = coordinator.notifier();

        notifier.request_dirty_flush();
        assert_eq!(started_rx.recv().await, Some(0));
        for _ in 0..32 {
            notifier.request_dirty_flush();
        }
        let drain = tokio::spawn(coordinator.drain());
        first_release
            .send(())
            .expect("first dirty flush is waiting for release");
        assert_eq!(started_rx.recv().await, Some(1));
        drain.await.expect("drain task joins");

        assert_eq!(dirty_calls.load(Ordering::SeqCst), 2);
        assert_eq!(full_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn dirty_notification_wakes_flush_consumer() {
        let (started, mut started_rx) = mpsc::channel(1);
        let (release, release_rx) = oneshot::channel();
        let mut release_rx = Some(release_rx);
        let coordinator = DirtyFlushCoordinator::spawn(move || {
            let started = started.clone();
            let release = release_rx.take().expect("one flush invocation is expected");
            async move {
                started.send(()).await.expect("test observes flush start");
                release.await.expect("test releases flush");
            }
        });

        coordinator.notifier().request();
        assert_eq!(started_rx.recv().await, Some(()));
        release.send(()).expect("flush is waiting for release");
        coordinator.drain().await;
    }

    #[tokio::test]
    async fn drain_waits_for_queued_flush_before_worker_stops() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (started, mut started_rx) = mpsc::channel(2);
        let (first_release, first_release_rx) = oneshot::channel();
        let mut first_release_rx = Some(first_release_rx);
        let coordinator = DirtyFlushCoordinator::spawn({
            let calls = Arc::clone(&calls);
            move || {
                let calls = Arc::clone(&calls);
                let started = started.clone();
                let first_release = first_release_rx.take();
                async move {
                    let call = calls.fetch_add(1, Ordering::SeqCst);
                    started.send(call).await.expect("test observes flush start");
                    if let Some(release) = first_release {
                        release.await.expect("test releases first flush");
                    }
                }
            }
        });
        let notifier = coordinator.notifier();

        notifier.request();
        assert_eq!(started_rx.recv().await, Some(0));
        notifier.request();
        let drain = tokio::spawn(coordinator.drain());
        first_release
            .send(())
            .expect("first flush is waiting for release");
        assert_eq!(started_rx.recv().await, Some(1));
        drain.await.expect("drain task joins");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn producer_notification_during_flush_is_not_missed() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (started, mut started_rx) = mpsc::channel(2);
        let (first_release, first_release_rx) = oneshot::channel();
        let mut first_release_rx = Some(first_release_rx);
        let coordinator = DirtyFlushCoordinator::spawn({
            let calls = Arc::clone(&calls);
            move || {
                let calls = Arc::clone(&calls);
                let started = started.clone();
                let first_release = first_release_rx.take();
                async move {
                    let call = calls.fetch_add(1, Ordering::SeqCst);
                    started.send(call).await.expect("test observes flush start");
                    if let Some(release) = first_release {
                        release.await.expect("test releases first flush");
                    }
                }
            }
        });
        let notifier = coordinator.notifier();

        notifier.request();
        assert_eq!(started_rx.recv().await, Some(0));
        notifier.request();
        first_release
            .send(())
            .expect("first flush is waiting for release");
        assert_eq!(started_rx.recv().await, Some(1));
        coordinator.drain().await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn drain_does_not_hang_after_worker_failure() {
        let (started, started_rx) = oneshot::channel();
        let mut started = Some(started);
        let coordinator = DirtyFlushCoordinator::spawn(move || {
            let started = started.take().expect("one flush invocation is expected");
            async move {
                started.send(()).expect("test observes worker start");
                panic!("injected dirty flush worker failure");
            }
        });

        coordinator.notifier().request();
        started_rx.await.expect("worker reports start");
        let drain = tokio::time::timeout(std::time::Duration::from_secs(1), coordinator.drain())
            .await
            .expect("worker failure resolves the drain");

        assert!(matches!(
            drain,
            DirtyFlushDrainOutcome::Failed(DirtyFlushDrainError::WorkerJoin(_))
        ));
    }
}
