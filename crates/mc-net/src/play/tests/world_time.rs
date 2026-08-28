use super::*;

#[test]
fn clientbound_session_world_time_separates_monotonic_and_overworld_clocks() {
    let sessions = SessionRegistry::new();
    sessions.set_world_time(12_345);
    sessions.advance_world_time(7);

    let packet = clientbound_session_world_time(&sessions);
    assert_eq!(packet.game_time, 7);
    assert_eq!(
        packet.overworld_clock,
        Some(mc_protocol::packets::play::WorldClockUpdate {
            total_ticks: 12_352,
            partial_tick: 0.0,
            rate: 1.0,
        })
    );

    sessions.set_daylight_cycle_enabled(false);
    sessions.advance_world_time(5);
    let frozen = clientbound_session_world_time(&sessions);
    assert_eq!(frozen.game_time, 12);
    assert_eq!(
        frozen.overworld_clock,
        Some(mc_protocol::packets::play::WorldClockUpdate {
            total_ticks: 12_352,
            partial_tick: 0.0,
            rate: 0.0,
        })
    );

    sessions.set_daylight_cycle_enabled(true);
    sessions.advance_world_time(3);
    assert_eq!(sessions.world_time(), 12_355);
    assert_eq!(sessions.simulation_tick(), 15);

    assert_eq!(clientbound_world_time(u64::MAX, 1, 1.0).game_time, i64::MAX);
}
