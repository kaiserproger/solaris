use super::client_load::ClientLoadGate;

#[test]
fn respawn_load_gate_accepts_acknowledgement_or_sixty_tick_timeout() {
    let mut acknowledged = ClientLoadGate::default();
    acknowledged.restart_after_respawn();
    assert!(!acknowledged.has_loaded());
    acknowledged.acknowledge();
    assert!(acknowledged.has_loaded());
    assert!(!acknowledged.tick());

    let mut timed_out = ClientLoadGate::default();
    timed_out.restart_after_respawn();
    for _ in 0..59 {
        assert!(!timed_out.tick());
        assert!(!timed_out.has_loaded());
    }
    assert!(timed_out.tick());
    assert!(timed_out.has_loaded());
}
