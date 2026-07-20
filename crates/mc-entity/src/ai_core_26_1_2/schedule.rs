const DAY_LENGTH: i64 = 24_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DayTime(pub(crate) i64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct GameTick(pub(crate) u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum VillagerSchedule {
    Adult,
    Baby,
}

/// A timetable result, not the villager brain's selected activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ScheduledActivity {
    Idle,
    Work,
    Play,
    Rest,
    Meet,
}

#[derive(Debug, Clone, Copy)]
struct ScheduleEntry {
    day_time: i64,
    activity: ScheduledActivity,
}

impl ScheduleEntry {
    const fn new(day_time: i64, activity: ScheduledActivity) -> Self {
        Self { day_time, activity }
    }
}

// Source for both tables: local extracted Java Edition 26.1.2 oracle at
// `.analysis/decompiled/server-26.1.2/data/minecraft/timeline/villager_schedule.json`.
const ADULT_VILLAGER_SCHEDULE: &[ScheduleEntry] = &[
    ScheduleEntry::new(10, ScheduledActivity::Idle),
    ScheduleEntry::new(2_000, ScheduledActivity::Work),
    ScheduleEntry::new(9_000, ScheduledActivity::Meet),
    ScheduleEntry::new(11_000, ScheduledActivity::Idle),
    ScheduleEntry::new(12_000, ScheduledActivity::Rest),
];

const BABY_VILLAGER_SCHEDULE: &[ScheduleEntry] = &[
    ScheduleEntry::new(10, ScheduledActivity::Idle),
    ScheduleEntry::new(3_000, ScheduledActivity::Play),
    ScheduleEntry::new(6_000, ScheduledActivity::Idle),
    ScheduleEntry::new(10_000, ScheduledActivity::Play),
    ScheduleEntry::new(12_000, ScheduledActivity::Rest),
];

pub(crate) fn villager_scheduled_activity(
    schedule: VillagerSchedule,
    day_time: DayTime,
) -> ScheduledActivity {
    let entries = match schedule {
        VillagerSchedule::Adult => ADULT_VILLAGER_SCHEDULE,
        VillagerSchedule::Baby => BABY_VILLAGER_SCHEDULE,
    };
    let normalized = day_time.0.rem_euclid(DAY_LENGTH);
    let mut selected = entries
        .last()
        .expect("villager schedule tables are nonempty")
        .activity;

    for entry in entries {
        if entry.day_time > normalized {
            break;
        }
        selected = entry.activity;
    }
    selected
}
