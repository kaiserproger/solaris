use super::{
    ScriptCommitEnqueueError, ScriptCommitEventMonitor, ScriptCommitEventOutbox,
    ScriptCommitEventReceiver,
};
use crate::ScriptEvent;
use std::sync::Arc;

fn outbox(
    capacity: usize,
) -> (
    ScriptCommitEventOutbox,
    ScriptCommitEventReceiver,
    Arc<ScriptCommitEventMonitor>,
) {
    let monitor = Arc::new(ScriptCommitEventMonitor::new(capacity));
    let (outbox, receiver) = monitor.channel();
    (outbox, receiver, monitor)
}

#[test]
fn required_overflow_is_fatal_and_preserves_accepted_event() {
    let (outbox, mut receiver, failure) = outbox(1);
    outbox.try_enqueue(ScriptEvent::server_started()).unwrap();
    assert_eq!(
        outbox.try_enqueue(ScriptEvent::server_stopping("overflow")),
        Err(ScriptCommitEnqueueError::RequiredOverflow)
    );
    assert!(failure.failed());
    assert_eq!(
        receiver.try_recv_required().unwrap(),
        ScriptEvent::server_started()
    );
    assert!(receiver.try_recv_required().is_none());
}

#[test]
fn receiving_releases_capacity_for_the_next_event() {
    let (outbox, mut receiver, failure) = outbox(1);
    outbox.try_enqueue(ScriptEvent::server_started()).unwrap();
    assert_eq!(
        receiver.try_recv_required().unwrap(),
        ScriptEvent::server_started()
    );
    outbox.try_enqueue(ScriptEvent::server_tick(2)).unwrap();
    assert_eq!(
        receiver.try_recv_required().unwrap(),
        ScriptEvent::server_tick(2)
    );
    assert!(!failure.failed());
}

#[test]
fn receiver_drop_with_backlog_fails_required_delivery() {
    let (outbox, receiver, failure) = outbox(2);
    outbox.try_enqueue(ScriptEvent::server_started()).unwrap();
    outbox.try_enqueue(ScriptEvent::server_tick(2)).unwrap();
    drop(receiver);
    assert!(failure.failed());
}

#[test]
fn empty_receiver_drop_does_not_report_delivery_failure() {
    let (_outbox, receiver, failure) = outbox(1);
    drop(receiver);
    assert!(!failure.failed());
}

#[test]
fn required_send_after_receiver_drop_is_fatal() {
    let (outbox, receiver, failure) = outbox(1);
    drop(receiver);
    assert_eq!(
        outbox.try_enqueue(ScriptEvent::server_started()),
        Err(ScriptCommitEnqueueError::RequiredClosed)
    );
    assert!(failure.failed());
}
