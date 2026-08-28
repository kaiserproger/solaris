pub const TICKS_PER_SECOND: i32 = 20;
pub const ON_FIRE_DAMAGE_INTERVAL_TICKS: i32 = 20;
pub const ON_FIRE_DAMAGE: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FireTick {
    pub remaining_ticks: i32,
    pub damage: f32,
}

#[must_use]
pub fn ignite_for_ticks(current: i32, requested: i32) -> i32 {
    current.max(requested.max(0))
}

#[must_use]
pub fn ignite_for_seconds(current: i32, seconds: f32) -> i32 {
    if !seconds.is_finite() || seconds <= 0.0 {
        return current;
    }
    let ticks = (seconds * TICKS_PER_SECOND as f32).floor();
    let ticks = ticks.clamp(0.0, i32::MAX as f32) as i32;
    ignite_for_ticks(current, ticks)
}

#[must_use]
pub fn tick_fire(remaining_ticks: i32, fire_immune: bool, in_lava: bool) -> FireTick {
    if remaining_ticks <= 0 {
        return FireTick {
            remaining_ticks,
            damage: 0.0,
        };
    }
    if fire_immune {
        return FireTick {
            remaining_ticks: 0,
            damage: 0.0,
        };
    }
    FireTick {
        remaining_ticks: remaining_ticks - 1,
        damage: if remaining_ticks % ON_FIRE_DAMAGE_INTERVAL_TICKS == 0 && !in_lava {
            ON_FIRE_DAMAGE
        } else {
            0.0
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignite_for_seconds_uses_floor_and_never_shortens_existing_fire() {
        assert_eq!(ignite_for_seconds(0, 5.0), 100);
        assert_eq!(ignite_for_seconds(120, 5.0), 120);
        assert_eq!(ignite_for_seconds(0, 0.049), 0);
        assert_eq!(ignite_for_seconds(0, 0.05), 1);
    }

    #[test]
    fn fire_ticks_damage_before_decrement_at_exact_twenty_tick_boundary() {
        assert_eq!(
            tick_fire(100, false, false),
            FireTick {
                remaining_ticks: 99,
                damage: 1.0,
            }
        );
        assert_eq!(
            tick_fire(99, false, false),
            FireTick {
                remaining_ticks: 98,
                damage: 0.0,
            }
        );
        assert_eq!(
            tick_fire(20, false, false),
            FireTick {
                remaining_ticks: 19,
                damage: 1.0,
            }
        );
    }

    #[test]
    fn lava_suppresses_on_fire_damage_but_still_consumes_timer() {
        assert_eq!(
            tick_fire(100, false, true),
            FireTick {
                remaining_ticks: 99,
                damage: 0.0,
            }
        );
    }

    #[test]
    fn fire_immunity_clears_positive_timer() {
        assert_eq!(
            tick_fire(100, true, false),
            FireTick {
                remaining_ticks: 0,
                damage: 0.0,
            }
        );
    }

    #[test]
    fn non_positive_timer_is_preserved_until_environment_changes_it() {
        assert_eq!(
            tick_fire(-20, false, false),
            FireTick {
                remaining_ticks: -20,
                damage: 0.0,
            }
        );
        assert_eq!(
            tick_fire(0, false, false),
            FireTick {
                remaining_ticks: 0,
                damage: 0.0,
            }
        );
    }
}
