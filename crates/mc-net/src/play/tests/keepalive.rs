use super::*;

#[test]
fn keepalive_tracker_only_accepts_the_matching_echo() {
    let mut keepalive = KeepAliveTracker::new();
    let request_id = keepalive.record_request().expect("first request");

    assert!(!keepalive.record_response(request_id + 1));
    assert_eq!(keepalive.pending_id(), Some(request_id));
    assert!(keepalive.record_response(request_id));
    assert_eq!(keepalive.pending_id(), None);
}

#[test]
fn keepalive_tracker_never_replaces_an_unanswered_request() {
    let mut keepalive = KeepAliveTracker::new();
    let request_id = keepalive.record_request().expect("first request");

    assert_eq!(keepalive.record_request(), None);
    assert_eq!(keepalive.pending_id(), Some(request_id));
    assert!(keepalive.record_response(request_id));
    assert!(keepalive.record_request().is_some());
}

#[test]
fn keepalive_timeout_requires_the_whole_connection_to_be_idle() {
    let mut keepalive = KeepAliveTracker::new();
    keepalive.record_request().expect("first request");
    keepalive.pending_since = Some(Instant::now() - KEEPALIVE_TIMEOUT - Duration::from_secs(1));

    keepalive.record_inbound_activity();
    assert_eq!(keepalive.timed_out(KEEPALIVE_TIMEOUT), None);

    keepalive.last_inbound_at = Instant::now() - KEEPALIVE_TIMEOUT - Duration::from_secs(1);
    assert!(keepalive.timed_out(KEEPALIVE_TIMEOUT).is_some());
}

#[test]
fn only_exact_recognized_packets_refresh_overdue_keepalive_activity() {
    let mut keepalive = KeepAliveTracker::new();
    keepalive.record_request().expect("first request");
    keepalive.pending_since = Some(Instant::now() - KEEPALIVE_TIMEOUT - Duration::from_secs(1));
    keepalive.last_inbound_at = Instant::now() - KEEPALIVE_TIMEOUT - Duration::from_secs(1);

    let unknown = liveness::validate_serverbound_play_frame(0x7fff, &Bytes::new()).unwrap();
    assert!(!unknown);
    assert!(keepalive.timed_out(KEEPALIVE_TIMEOUT).is_some());

    let mut valid = BytesMut::new();
    ConfirmTeleportation { teleport_id: 7 }
        .encode(&mut valid)
        .unwrap();
    assert!(
        liveness::validate_serverbound_play_frame(ConfirmTeleportation::ID, &valid.freeze())
            .unwrap()
    );
    keepalive.record_inbound_activity();
    assert_eq!(keepalive.timed_out(KEEPALIVE_TIMEOUT), None);
}
