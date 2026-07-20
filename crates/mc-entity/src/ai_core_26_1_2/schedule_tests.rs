use super::schedule::{
    DayTime, GameTick, ScheduledActivity, VillagerSchedule, villager_scheduled_activity,
};

#[test]
fn adult_schedule_matches_oracle_boundaries() {
    let cases = [
        (0, ScheduledActivity::Rest),
        (9, ScheduledActivity::Rest),
        (10, ScheduledActivity::Idle),
        (1_999, ScheduledActivity::Idle),
        (2_000, ScheduledActivity::Work),
        (8_999, ScheduledActivity::Work),
        (9_000, ScheduledActivity::Meet),
        (10_999, ScheduledActivity::Meet),
        (11_000, ScheduledActivity::Idle),
        (11_999, ScheduledActivity::Idle),
        (12_000, ScheduledActivity::Rest),
        (23_999, ScheduledActivity::Rest),
    ];

    for (day_time, expected) in cases {
        assert_eq!(
            villager_scheduled_activity(VillagerSchedule::Adult, DayTime(day_time)),
            expected,
            "unexpected adult activity at day time {day_time}"
        );
    }
}

#[test]
fn baby_schedule_matches_oracle_boundaries() {
    let cases = [
        (0, ScheduledActivity::Rest),
        (9, ScheduledActivity::Rest),
        (10, ScheduledActivity::Idle),
        (2_999, ScheduledActivity::Idle),
        (3_000, ScheduledActivity::Play),
        (5_999, ScheduledActivity::Play),
        (6_000, ScheduledActivity::Idle),
        (9_999, ScheduledActivity::Idle),
        (10_000, ScheduledActivity::Play),
        (11_999, ScheduledActivity::Play),
        (12_000, ScheduledActivity::Rest),
        (23_999, ScheduledActivity::Rest),
    ];

    for (day_time, expected) in cases {
        assert_eq!(
            villager_scheduled_activity(VillagerSchedule::Baby, DayTime(day_time)),
            expected,
            "unexpected baby activity at day time {day_time}"
        );
    }
}

#[test]
fn day_time_wraps_and_normalizes_negative_values() {
    let cases = [
        (24_000, ScheduledActivity::Rest),
        (24_010, ScheduledActivity::Idle),
        (-1, ScheduledActivity::Rest),
        (-23_990, ScheduledActivity::Idle),
        (-24_000, ScheduledActivity::Rest),
    ];

    for (day_time, expected) in cases {
        assert_eq!(
            villager_scheduled_activity(VillagerSchedule::Adult, DayTime(day_time)),
            expected,
            "unexpected wrapped activity at day time {day_time}"
        );
    }
}

#[test]
fn scheduled_activity_is_independent_of_game_tick() {
    fn caller_lookup(game_tick: GameTick, day_time: DayTime) -> ScheduledActivity {
        let GameTick(_observed_tick) = game_tick;
        villager_scheduled_activity(VillagerSchedule::Adult, day_time)
    }

    assert_eq!(
        caller_lookup(GameTick(0), DayTime(2_000)),
        caller_lookup(GameTick(u64::MAX), DayTime(2_000))
    );
}

#[test]
fn higher_precedence_unscheduled_activity_stays_caller_owned() {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CallerActivity {
        Panic,
        Combat,
        Raid,
        Scheduled(ScheduledActivity),
    }

    fn select_activity(
        higher_precedence: Option<CallerActivity>,
        scheduled: ScheduledActivity,
    ) -> CallerActivity {
        higher_precedence.unwrap_or(CallerActivity::Scheduled(scheduled))
    }

    let scheduled = villager_scheduled_activity(VillagerSchedule::Adult, DayTime(9_000));
    for higher_precedence in [
        CallerActivity::Panic,
        CallerActivity::Combat,
        CallerActivity::Raid,
    ] {
        let selected = select_activity(Some(higher_precedence), scheduled);
        assert_eq!(selected, higher_precedence);
    }
    assert_eq!(
        select_activity(None, scheduled),
        CallerActivity::Scheduled(ScheduledActivity::Meet)
    );
}
