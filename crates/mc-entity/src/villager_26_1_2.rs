//! Data-driven villager brain policy for the Java Edition 26.1.2 target.
//!
//! The module is pure: callers provide an authoritative persisted brain snapshot,
//! lifecycle/day clocks, and a bounded profile. Planning never reads the world or
//! mutates ECS state. Regional authority commits the returned state + goal through
//! the ordinary snapshot CAS path.

use serde::{Deserialize, Serialize};

use crate::{EntityId, GoalState, Vec3};

const DAY_LENGTH: i64 = 24_000;
const MAX_SCHEDULE_ENTRIES: usize = 32;
const MAX_WALK_SPEED: f64 = 4.0;
const MAX_WANDER_PERIOD_TICKS: u32 = 24_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VillagerScheduleKind {
    Adult,
    Baby,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VillagerActivity {
    Idle,
    Work,
    Play,
    Rest,
    Meet,
    Controlled,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum VillagerBrainOverride {
    Hold,
    FollowPosition { target: Vec3, speed: f64 },
    Wander { speed: f64, period_ticks: u32 },
}

impl VillagerBrainOverride {
    fn validate(self) -> Result<(), VillagerBrainError> {
        match self {
            Self::Hold => Ok(()),
            Self::FollowPosition { target, speed } => {
                validate_position(target)?;
                validate_speed(speed)
            }
            Self::Wander {
                speed,
                period_ticks,
            } => {
                validate_speed(speed)?;
                if period_ticks == 0 || period_ticks > MAX_WANDER_PERIOD_TICKS {
                    return Err(VillagerBrainError::InvalidWanderPeriod(period_ticks));
                }
                Ok(())
            }
        }
    }

    fn goal(self) -> GoalState {
        match self {
            Self::Hold => GoalState::Idle,
            Self::FollowPosition { target, speed } => GoalState::FollowPosition { target, speed },
            Self::Wander {
                speed,
                period_ticks,
            } => GoalState::Wander {
                speed,
                period_ticks,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct VillagerPoiSet {
    pub home: Option<Vec3>,
    pub job_site: Option<Vec3>,
    pub meeting_point: Option<Vec3>,
}

impl VillagerPoiSet {
    pub fn validate(self) -> Result<(), VillagerBrainError> {
        for (kind, position) in [
            (VillagerPoiKind::Home, self.home),
            (VillagerPoiKind::JobSite, self.job_site),
            (VillagerPoiKind::MeetingPoint, self.meeting_point),
        ] {
            if let Some(position) = position {
                validate_position(position).map_err(|_| VillagerBrainError::InvalidPoi(kind))?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VillagerPoiKind {
    Home,
    JobSite,
    MeetingPoint,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VillagerBrainState {
    pub schedule: VillagerScheduleKind,
    pub activity: VillagerActivity,
    pub pois: VillagerPoiSet,
    pub override_order: Option<VillagerBrainOverride>,
    pub override_expires_tick: Option<u64>,
    /// Bounded 26.1.2 `LAST_SLEPT` projection. Until villager bed pose is modeled,
    /// Solaris records this when a Rest-scheduled villager with a home POI leaves Rest.
    #[serde(default)]
    pub last_slept_tick: Option<u64>,
    /// Inclusive lifecycle tick through which `GOLEM_DETECTED_RECENTLY` is present.
    #[serde(default)]
    pub golem_detected_until_tick: Option<u64>,
    #[serde(skip)]
    pub interaction_target: Option<EntityId>,
    #[serde(skip)]
    pub last_gossip_time: u64,
}

impl VillagerBrainState {
    #[must_use]
    pub const fn adult(pois: VillagerPoiSet) -> Self {
        Self {
            schedule: VillagerScheduleKind::Adult,
            activity: VillagerActivity::Idle,
            pois,
            override_order: None,
            override_expires_tick: None,
            last_slept_tick: None,
            golem_detected_until_tick: None,
            interaction_target: None,
            last_gossip_time: 0,
        }
    }

    #[must_use]
    pub const fn baby(pois: VillagerPoiSet) -> Self {
        Self {
            schedule: VillagerScheduleKind::Baby,
            activity: VillagerActivity::Idle,
            pois,
            override_order: None,
            override_expires_tick: None,
            last_slept_tick: None,
            golem_detected_until_tick: None,
            interaction_target: None,
            last_gossip_time: 0,
        }
    }

    pub fn set_override(
        &mut self,
        order: VillagerBrainOverride,
        current_tick: u64,
        expires_tick: u64,
    ) -> Result<(), VillagerBrainError> {
        order.validate()?;
        if expires_tick <= current_tick {
            return Err(VillagerBrainError::ExpiredOverride);
        }
        self.override_order = Some(order);
        self.override_expires_tick = Some(expires_tick);
        Ok(())
    }

    pub fn clear_override(&mut self) {
        self.override_order = None;
        self.override_expires_tick = None;
    }

    #[must_use]
    pub fn recently_slept(&self, current_tick: u64) -> bool {
        self.last_slept_tick
            .is_some_and(|last| current_tick.saturating_sub(last) < 24_000)
    }

    #[must_use]
    pub fn golem_detected_recently(&self, current_tick: u64) -> bool {
        self.golem_detected_until_tick
            .is_some_and(|expires| current_tick <= expires)
    }

    pub fn note_golem_detected(&mut self, current_tick: u64) {
        self.golem_detected_until_tick = Some(current_tick.saturating_add(599));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct VillagerScheduleEntry {
    pub day_time: i64,
    pub activity: VillagerActivity,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VillagerBrainProfile {
    pub adult_schedule: Vec<VillagerScheduleEntry>,
    pub baby_schedule: Vec<VillagerScheduleEntry>,
    pub idle_wander_speed: f64,
    pub idle_wander_period_ticks: u32,
    pub play_wander_speed: f64,
    pub play_wander_period_ticks: u32,
    pub work_speed: f64,
    pub meet_speed: f64,
    pub rest_speed: f64,
}

impl VillagerBrainProfile {
    #[must_use]
    pub fn vanilla_26_1_2() -> Self {
        Self {
            adult_schedule: vec![
                VillagerScheduleEntry {
                    day_time: 10,
                    activity: VillagerActivity::Idle,
                },
                VillagerScheduleEntry {
                    day_time: 2_000,
                    activity: VillagerActivity::Work,
                },
                VillagerScheduleEntry {
                    day_time: 9_000,
                    activity: VillagerActivity::Meet,
                },
                VillagerScheduleEntry {
                    day_time: 11_000,
                    activity: VillagerActivity::Idle,
                },
                VillagerScheduleEntry {
                    day_time: 12_000,
                    activity: VillagerActivity::Rest,
                },
            ],
            baby_schedule: vec![
                VillagerScheduleEntry {
                    day_time: 10,
                    activity: VillagerActivity::Idle,
                },
                VillagerScheduleEntry {
                    day_time: 3_000,
                    activity: VillagerActivity::Play,
                },
                VillagerScheduleEntry {
                    day_time: 6_000,
                    activity: VillagerActivity::Idle,
                },
                VillagerScheduleEntry {
                    day_time: 10_000,
                    activity: VillagerActivity::Play,
                },
                VillagerScheduleEntry {
                    day_time: 12_000,
                    activity: VillagerActivity::Rest,
                },
            ],
            idle_wander_speed: 0.3,
            idle_wander_period_ticks: 80,
            play_wander_speed: 0.36,
            play_wander_period_ticks: 40,
            work_speed: 0.3,
            meet_speed: 0.3,
            rest_speed: 0.3,
        }
    }

    pub fn validate(&self) -> Result<(), VillagerBrainError> {
        validate_schedule(&self.adult_schedule, VillagerScheduleKind::Adult)?;
        validate_schedule(&self.baby_schedule, VillagerScheduleKind::Baby)?;
        for speed in [
            self.idle_wander_speed,
            self.play_wander_speed,
            self.work_speed,
            self.meet_speed,
            self.rest_speed,
        ] {
            validate_speed(speed)?;
        }
        for period in [self.idle_wander_period_ticks, self.play_wander_period_ticks] {
            if period == 0 || period > MAX_WANDER_PERIOD_TICKS {
                return Err(VillagerBrainError::InvalidWanderPeriod(period));
            }
        }
        Ok(())
    }

    pub fn validated(&self) -> Result<ValidatedVillagerBrainProfile<'_>, VillagerBrainError> {
        self.validate()?;
        Ok(ValidatedVillagerBrainProfile(self))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ValidatedVillagerBrainProfile<'a>(&'a VillagerBrainProfile);

impl ValidatedVillagerBrainProfile<'_> {
    pub fn plan(
        self,
        current: &VillagerBrainState,
        lifecycle_tick: u64,
        day_time: i64,
    ) -> Result<VillagerBrainPlan, VillagerBrainError> {
        current.pois.validate()?;
        plan_villager_brain_validated(current, self.0, lifecycle_tick, day_time)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VillagerBrainPlan {
    pub state: VillagerBrainState,
    pub goal: GoalState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VillagerBrainError {
    EmptySchedule(VillagerScheduleKind),
    TooManyScheduleEntries(VillagerScheduleKind),
    InvalidScheduleOrder(VillagerScheduleKind),
    UnsupportedScheduledActivity(VillagerActivity),
    InvalidSpeed,
    InvalidWanderPeriod(u32),
    InvalidPoi(VillagerPoiKind),
    InvalidOverride,
    ExpiredOverride,
}

pub fn plan_villager_brain(
    current: &VillagerBrainState,
    profile: &VillagerBrainProfile,
    lifecycle_tick: u64,
    day_time: i64,
) -> Result<VillagerBrainPlan, VillagerBrainError> {
    profile.validated()?.plan(current, lifecycle_tick, day_time)
}

fn plan_villager_brain_validated(
    current: &VillagerBrainState,
    profile: &VillagerBrainProfile,
    lifecycle_tick: u64,
    day_time: i64,
) -> Result<VillagerBrainPlan, VillagerBrainError> {
    let mut next = current.clone();
    if next
        .override_expires_tick
        .is_some_and(|expires| lifecycle_tick >= expires)
    {
        next.clear_override();
    }

    if let Some(order) = next.override_order {
        order
            .validate()
            .map_err(|_| VillagerBrainError::InvalidOverride)?;
        next.activity = VillagerActivity::Controlled;
        return Ok(VillagerBrainPlan {
            state: next,
            goal: order.goal(),
        });
    }

    let previous_activity = next.activity;
    let scheduled = scheduled_activity(profile, next.schedule, day_time);
    let (activity, goal) = scheduled_goal(scheduled, next.pois, profile);
    if previous_activity == VillagerActivity::Rest
        && activity != VillagerActivity::Rest
        && next.pois.home.is_some()
    {
        next.last_slept_tick = Some(lifecycle_tick);
    }
    next.activity = activity;
    Ok(VillagerBrainPlan { state: next, goal })
}

fn scheduled_activity(
    profile: &VillagerBrainProfile,
    schedule: VillagerScheduleKind,
    day_time: i64,
) -> VillagerActivity {
    let entries = match schedule {
        VillagerScheduleKind::Adult => &profile.adult_schedule,
        VillagerScheduleKind::Baby => &profile.baby_schedule,
    };
    let normalized = day_time.rem_euclid(DAY_LENGTH);
    let mut selected = entries
        .last()
        .expect("validated schedule is nonempty")
        .activity;
    for entry in entries {
        if entry.day_time > normalized {
            break;
        }
        selected = entry.activity;
    }
    selected
}

fn scheduled_goal(
    activity: VillagerActivity,
    pois: VillagerPoiSet,
    profile: &VillagerBrainProfile,
) -> (VillagerActivity, GoalState) {
    match activity {
        VillagerActivity::Work => poi_goal(
            activity,
            pois.job_site,
            profile.work_speed,
            profile.idle_wander_speed,
            profile.idle_wander_period_ticks,
        ),
        VillagerActivity::Meet => poi_goal(
            activity,
            pois.meeting_point,
            profile.meet_speed,
            profile.idle_wander_speed,
            profile.idle_wander_period_ticks,
        ),
        VillagerActivity::Rest => poi_goal(
            activity,
            pois.home,
            profile.rest_speed,
            profile.idle_wander_speed,
            profile.idle_wander_period_ticks,
        ),
        VillagerActivity::Play => (
            VillagerActivity::Play,
            GoalState::Wander {
                speed: profile.play_wander_speed,
                period_ticks: profile.play_wander_period_ticks,
            },
        ),
        VillagerActivity::Idle => (
            VillagerActivity::Idle,
            GoalState::Wander {
                speed: profile.idle_wander_speed,
                period_ticks: profile.idle_wander_period_ticks,
            },
        ),
        VillagerActivity::Controlled => unreachable!("controlled is not schedulable"),
    }
}

fn poi_goal(
    activity: VillagerActivity,
    position: Option<Vec3>,
    speed: f64,
    fallback_speed: f64,
    fallback_period: u32,
) -> (VillagerActivity, GoalState) {
    position.map_or_else(
        || {
            (
                VillagerActivity::Idle,
                GoalState::Wander {
                    speed: fallback_speed,
                    period_ticks: fallback_period,
                },
            )
        },
        |target| (activity, GoalState::FollowPosition { target, speed }),
    )
}

fn validate_schedule(
    entries: &[VillagerScheduleEntry],
    kind: VillagerScheduleKind,
) -> Result<(), VillagerBrainError> {
    if entries.is_empty() {
        return Err(VillagerBrainError::EmptySchedule(kind));
    }
    if entries.len() > MAX_SCHEDULE_ENTRIES {
        return Err(VillagerBrainError::TooManyScheduleEntries(kind));
    }
    let mut previous = None;
    for entry in entries {
        if !(0..DAY_LENGTH).contains(&entry.day_time)
            || previous.is_some_and(|previous| entry.day_time <= previous)
        {
            return Err(VillagerBrainError::InvalidScheduleOrder(kind));
        }
        if entry.activity == VillagerActivity::Controlled {
            return Err(VillagerBrainError::UnsupportedScheduledActivity(
                entry.activity,
            ));
        }
        previous = Some(entry.day_time);
    }
    Ok(())
}

fn validate_speed(speed: f64) -> Result<(), VillagerBrainError> {
    if !speed.is_finite() || speed <= 0.0 || speed > MAX_WALK_SPEED {
        return Err(VillagerBrainError::InvalidSpeed);
    }
    Ok(())
}

fn validate_position(position: Vec3) -> Result<(), VillagerBrainError> {
    if position.is_finite() {
        Ok(())
    } else {
        Err(VillagerBrainError::InvalidOverride)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pois() -> VillagerPoiSet {
        VillagerPoiSet {
            home: Some(Vec3::new(1.0, 64.0, 1.0)),
            job_site: Some(Vec3::new(8.0, 64.0, 1.0)),
            meeting_point: Some(Vec3::new(4.0, 64.0, 4.0)),
        }
    }

    #[test]
    fn vanilla_adult_schedule_selects_exact_poi_goals_and_wraps() {
        let profile = VillagerBrainProfile::vanilla_26_1_2();
        let state = VillagerBrainState::adult(pois());
        let cases = [
            (0, VillagerActivity::Rest, pois().home.unwrap()),
            (2_000, VillagerActivity::Work, pois().job_site.unwrap()),
            (9_000, VillagerActivity::Meet, pois().meeting_point.unwrap()),
            (12_000, VillagerActivity::Rest, pois().home.unwrap()),
            (26_000, VillagerActivity::Work, pois().job_site.unwrap()),
        ];
        for (day_time, expected_activity, target) in cases {
            let plan = plan_villager_brain(&state, &profile, 1, day_time).unwrap();
            assert_eq!(plan.state.activity, expected_activity);
            assert_eq!(plan.goal, GoalState::FollowPosition { target, speed: 0.3 });
        }
    }

    #[test]
    fn missing_poi_falls_back_to_idle_wander_without_fake_position() {
        let profile = VillagerBrainProfile::vanilla_26_1_2();
        let state = VillagerBrainState::adult(VillagerPoiSet::default());
        let plan = plan_villager_brain(&state, &profile, 1, 2_000).unwrap();
        assert_eq!(plan.state.activity, VillagerActivity::Idle);
        assert_eq!(
            plan.goal,
            GoalState::Wander {
                speed: profile.idle_wander_speed,
                period_ticks: profile.idle_wander_period_ticks,
            }
        );
    }

    #[test]
    fn override_preempts_schedule_and_expires_on_exact_lifecycle_tick() {
        let profile = VillagerBrainProfile::vanilla_26_1_2();
        let mut state = VillagerBrainState::adult(pois());
        state
            .set_override(VillagerBrainOverride::Hold, 0, 100)
            .unwrap();
        let before = plan_villager_brain(&state, &profile, 99, 2_000).unwrap();
        assert_eq!(before.state.activity, VillagerActivity::Controlled);
        assert_eq!(before.goal, GoalState::Idle);
        let expired = plan_villager_brain(&before.state, &profile, 100, 2_000).unwrap();
        assert_eq!(expired.state.activity, VillagerActivity::Work);
        assert_eq!(expired.state.override_order, None);
        assert_eq!(
            expired.goal,
            GoalState::FollowPosition {
                target: pois().job_site.unwrap(),
                speed: 0.3,
            }
        );
    }

    #[test]
    fn custom_profile_is_validated_and_changes_planning_without_runtime_branching() {
        let mut profile = VillagerBrainProfile::vanilla_26_1_2();
        profile.work_speed = 0.75;
        profile.adult_schedule = vec![VillagerScheduleEntry {
            day_time: 0,
            activity: VillagerActivity::Work,
        }];
        let plan =
            plan_villager_brain(&VillagerBrainState::adult(pois()), &profile, 1, 12_000).unwrap();
        assert_eq!(
            plan.goal,
            GoalState::FollowPosition {
                target: pois().job_site.unwrap(),
                speed: 0.75,
            }
        );

        profile.adult_schedule.push(VillagerScheduleEntry {
            day_time: 0,
            activity: VillagerActivity::Rest,
        });
        assert_eq!(
            profile.validate(),
            Err(VillagerBrainError::InvalidScheduleOrder(
                VillagerScheduleKind::Adult
            ))
        );
    }

    #[test]
    fn non_finite_poi_and_override_fail_closed() {
        let profile = VillagerBrainProfile::vanilla_26_1_2();
        let state = VillagerBrainState::adult(VillagerPoiSet {
            home: Some(Vec3::new(f64::NAN, 64.0, 0.0)),
            ..VillagerPoiSet::default()
        });
        assert_eq!(
            plan_villager_brain(&state, &profile, 1, 0),
            Err(VillagerBrainError::InvalidPoi(VillagerPoiKind::Home))
        );

        let mut state = VillagerBrainState::adult(pois());
        assert_eq!(
            state.set_override(
                VillagerBrainOverride::FollowPosition {
                    target: Vec3::new(0.0, 64.0, 0.0),
                    speed: f64::INFINITY,
                },
                0,
                10,
            ),
            Err(VillagerBrainError::InvalidSpeed)
        );
    }

    #[test]
    fn leaving_rest_records_exact_sleep_tick_and_recent_sleep_boundary() {
        let profile = VillagerBrainProfile::vanilla_26_1_2();
        let mut state = VillagerBrainState::adult(pois());
        state.activity = VillagerActivity::Rest;

        let plan = plan_villager_brain(&state, &profile, 123, 2_000).unwrap();
        assert_eq!(plan.state.activity, VillagerActivity::Work);
        assert_eq!(plan.state.last_slept_tick, Some(123));
        assert!(plan.state.recently_slept(24_122));
        assert!(!plan.state.recently_slept(24_123));
    }

    #[test]
    fn golem_detection_memory_expires_after_exact_599_tick_ttl() {
        let mut state = VillagerBrainState::adult(pois());
        state.note_golem_detected(100);

        assert_eq!(state.golem_detected_until_tick, Some(699));
        assert!(state.golem_detected_recently(699));
        assert!(!state.golem_detected_recently(700));
    }
}
