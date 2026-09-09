//! Bounded required delivery for events whose gameplay changes already committed.
//! Queue failure is fatal; networking and gameplay authority remain caller-owned.

use crate::ScriptEvent;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::sync::{mpsc, watch};

const SCRIPT_COMMIT_EVENT_OUTBOX_CAPACITY: usize = 256;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScriptCommitEventOutboxSnapshot {
    pub capacity: usize,
    pub depth: usize,
    pub max_depth: usize,
    pub enqueued: u64,
    pub dequeued: u64,
    pub required_overflow: u64,
    pub required_closed: u64,
    pub required_abandoned_on_receiver_drop: u64,
}

#[derive(Debug)]
pub struct ScriptCommitEventMonitor {
    capacity: usize,
    depth: AtomicUsize,
    max_depth: AtomicUsize,
    enqueued: AtomicU64,
    dequeued: AtomicU64,
    required_overflow: AtomicU64,
    required_closed: AtomicU64,
    required_abandoned_on_receiver_drop: AtomicU64,
    required_failure: watch::Sender<bool>,
}

impl Default for ScriptCommitEventMonitor {
    fn default() -> Self {
        Self::new(SCRIPT_COMMIT_EVENT_OUTBOX_CAPACITY)
    }
}

impl ScriptCommitEventMonitor {
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            depth: AtomicUsize::new(0),
            max_depth: AtomicUsize::new(0),
            enqueued: AtomicU64::new(0),
            dequeued: AtomicU64::new(0),
            required_overflow: AtomicU64::new(0),
            required_closed: AtomicU64::new(0),
            required_abandoned_on_receiver_drop: AtomicU64::new(0),
            required_failure: watch::channel(false).0,
        }
    }

    pub fn channel(self: &Arc<Self>) -> (ScriptCommitEventOutbox, ScriptCommitEventReceiver) {
        let (sender, receiver) = mpsc::channel(self.capacity);
        (
            ScriptCommitEventOutbox {
                sender,
                monitor: Arc::clone(self),
            },
            ScriptCommitEventReceiver {
                receiver,
                monitor: Arc::clone(self),
            },
        )
    }

    pub fn failed(&self) -> bool {
        *self.required_failure.borrow()
    }

    pub async fn wait_for_failure(&self) {
        let mut failure = self.required_failure.subscribe();
        // The borrowed monitor retains the sender while this future is alive.
        let _ = failure.wait_for(|failed| *failed).await;
    }
}

impl ScriptCommitEventMonitor {
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

    pub fn snapshot(&self) -> ScriptCommitEventOutboxSnapshot {
        ScriptCommitEventOutboxSnapshot {
            capacity: self.capacity,
            depth: self.depth.load(Ordering::Relaxed),
            max_depth: self.max_depth.load(Ordering::Relaxed),
            enqueued: self.enqueued.load(Ordering::Relaxed),
            dequeued: self.dequeued.load(Ordering::Relaxed),
            required_overflow: self.required_overflow.load(Ordering::Relaxed),
            required_closed: self.required_closed.load(Ordering::Relaxed),
            required_abandoned_on_receiver_drop: self
                .required_abandoned_on_receiver_drop
                .load(Ordering::Relaxed),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptCommitEnqueueError {
    RequiredOverflow,
    RequiredClosed,
}

#[derive(Debug, Clone)]
pub struct ScriptCommitEventOutbox {
    sender: mpsc::Sender<ScriptEvent>,
    monitor: Arc<ScriptCommitEventMonitor>,
}

impl ScriptCommitEventOutbox {
    pub fn try_enqueue(&self, event: ScriptEvent) -> Result<(), ScriptCommitEnqueueError> {
        match self.sender.try_reserve() {
            Ok(permit) => {
                self.monitor.enqueued.fetch_add(1, Ordering::Relaxed);
                let depth = self.monitor.depth.fetch_add(1, Ordering::Relaxed) + 1;
                self.monitor.record_depth(depth);
                permit.send(event);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.monitor
                    .required_overflow
                    .fetch_add(1, Ordering::Relaxed);
                self.monitor.required_failure.send_replace(true);
                Err(ScriptCommitEnqueueError::RequiredOverflow)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {
                self.monitor.required_closed.fetch_add(1, Ordering::Relaxed);
                self.monitor.required_failure.send_replace(true);
                Err(ScriptCommitEnqueueError::RequiredClosed)
            }
        }
    }
}

pub struct ScriptCommitEventReceiver {
    receiver: mpsc::Receiver<ScriptEvent>,
    monitor: Arc<ScriptCommitEventMonitor>,
}

impl Drop for ScriptCommitEventReceiver {
    fn drop(&mut self) {
        self.receiver.close();
        let mut abandoned = 0_usize;
        while self.receiver.try_recv().is_ok() {
            abandoned = abandoned.saturating_add(1);
        }
        if abandoned == 0 {
            return;
        }
        self.monitor.depth.fetch_sub(abandoned, Ordering::Relaxed);
        self.monitor.required_abandoned_on_receiver_drop.fetch_add(
            u64::try_from(abandoned).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.monitor.required_failure.send_replace(true);
    }
}

impl ScriptCommitEventReceiver {
    pub async fn recv(&mut self) -> Option<ScriptEvent> {
        let event = self.receiver.recv().await?;
        self.monitor.depth.fetch_sub(1, Ordering::Relaxed);
        self.monitor.dequeued.fetch_add(1, Ordering::Relaxed);
        Some(event)
    }

    pub fn try_recv_required(&mut self) -> Option<ScriptEvent> {
        let event = self.receiver.try_recv().ok()?;
        self.monitor.depth.fetch_sub(1, Ordering::Relaxed);
        self.monitor.dequeued.fetch_add(1, Ordering::Relaxed);
        Some(event)
    }

    pub fn report_required_failure(&self) {
        self.monitor.required_failure.send_replace(true);
    }
}

#[cfg(test)]
#[path = "commit_events_tests.rs"]
mod tests;
