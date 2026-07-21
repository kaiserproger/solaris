use std::num::NonZeroUsize;

use super::*;

fn nonzero(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).unwrap()
}

#[test]
fn latest_server_tick_is_coalesced_behind_a_full_event_queue() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let buffered = ScriptEvent::server_started();
    boundary.try_enqueue_event(buffered.clone()).unwrap();

    boundary.try_enqueue_latest_server_tick(7).unwrap();
    boundary.try_enqueue_latest_server_tick(9).unwrap();
    boundary.try_enqueue_latest_server_tick(8).unwrap();

    assert_eq!(endpoint.recv_event_blocking(), Some(buffered));
    assert_eq!(
        endpoint.recv_event_blocking(),
        Some(ScriptEvent::server_tick(9))
    );
}

#[test]
fn newly_sendable_tick_merges_an_older_coalesced_tick_without_reordering() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let buffered = ScriptEvent::server_started();
    boundary.try_enqueue_event(buffered.clone()).unwrap();
    boundary.try_enqueue_latest_server_tick(9).unwrap();
    assert_eq!(endpoint.recv_event_blocking(), Some(buffered));

    boundary.try_enqueue_latest_server_tick(10).unwrap();
    boundary.close_event_admission();

    assert_eq!(
        endpoint.recv_event_blocking(),
        Some(ScriptEvent::server_tick(10))
    );
    assert_eq!(endpoint.recv_event_blocking(), None);
}

#[test]
fn coalesced_tick_runs_before_an_event_admitted_after_queue_progress() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let before_tick = ScriptEvent::server_started();
    let after_tick = ScriptEvent::server_stopping("later");
    boundary.try_enqueue_event(before_tick.clone()).unwrap();
    boundary.try_enqueue_latest_server_tick(30).unwrap();
    assert_eq!(endpoint.recv_event_blocking(), Some(before_tick));
    boundary.try_enqueue_event(after_tick.clone()).unwrap();

    assert_eq!(
        endpoint.recv_event_blocking(),
        Some(ScriptEvent::server_tick(30))
    );
    assert_eq!(endpoint.recv_event_blocking(), Some(after_tick));
}

#[test]
fn late_smaller_tick_is_ignored_after_a_newer_tick_was_admitted() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(2), nonzero(1));
    boundary.try_enqueue_latest_server_tick(10).unwrap();
    boundary.try_enqueue_latest_server_tick(9).unwrap();
    boundary.close_event_admission();

    assert_eq!(
        endpoint.recv_event_blocking(),
        Some(ScriptEvent::server_tick(10))
    );
    assert_eq!(endpoint.recv_event_blocking(), None);
}

#[test]
fn pending_newer_tick_suppresses_an_older_tick_already_in_the_channel() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(2), nonzero(1));
    let ordinary = ScriptEvent::server_started();
    boundary.try_enqueue_event(ordinary.clone()).unwrap();
    boundary.try_enqueue_latest_server_tick(10).unwrap();
    boundary.try_enqueue_latest_server_tick(11).unwrap();
    boundary.close_event_admission();

    assert_eq!(endpoint.recv_event_blocking(), Some(ordinary));
    assert_eq!(
        endpoint.recv_event_blocking(),
        Some(ScriptEvent::server_tick(11))
    );
    assert_eq!(endpoint.recv_event_blocking(), None);
}

#[tokio::test]
async fn asynchronous_host_receive_drains_the_coalesced_tick() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let buffered = ScriptEvent::server_started();
    boundary.try_enqueue_event(buffered.clone()).unwrap();
    boundary.try_enqueue_latest_server_tick(17).unwrap();

    assert_eq!(endpoint.recv_event().await, Some(buffered));
    assert_eq!(
        endpoint.recv_event().await,
        Some(ScriptEvent::server_tick(17))
    );
}

#[test]
fn server_tick_uses_the_normal_event_channel_when_capacity_is_available() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));

    boundary.try_enqueue_latest_server_tick(41).unwrap();

    assert_eq!(
        endpoint.recv_event_blocking(),
        Some(ScriptEvent::server_tick(41))
    );
}

#[test]
fn closed_event_admission_rejects_latest_server_tick_without_retaining_it() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    boundary.close_event_admission();

    assert_eq!(
        boundary.try_enqueue_latest_server_tick(1),
        Err(ScriptQueueError::Closed)
    );
    assert_eq!(endpoint.recv_event_blocking(), None);
}

#[test]
fn closing_event_admission_drains_an_already_coalesced_tick() {
    let (boundary, mut endpoint) = script_boundary_pair(nonzero(1), nonzero(1));
    let buffered = ScriptEvent::server_started();
    boundary.try_enqueue_event(buffered.clone()).unwrap();
    boundary.try_enqueue_latest_server_tick(23).unwrap();
    boundary.close_event_admission();

    assert_eq!(endpoint.recv_event_blocking(), Some(buffered));
    assert_eq!(
        endpoint.recv_event_blocking(),
        Some(ScriptEvent::server_tick(23))
    );
    assert_eq!(endpoint.recv_event_blocking(), None);
}
