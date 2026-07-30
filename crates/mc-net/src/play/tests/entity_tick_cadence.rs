use std::time::Duration;

use super::{ENTITY_MOVE_SEND_INTERVAL_TICKS, ENTITY_TICK_PERIOD};

#[test]
fn entity_tick_cadence_matches_vanilla_cow_tracking() {
    assert_eq!(ENTITY_TICK_PERIOD, Duration::from_millis(50));
    assert_eq!(mc_physics::TICK_SECONDS, 0.05);
    assert_eq!(ENTITY_MOVE_SEND_INTERVAL_TICKS, 3);
}
