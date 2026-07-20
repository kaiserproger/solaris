//! Pure AI policy fragments for Solaris' Java Edition 26.1.2 target.
//!
//! This module plans goal-control transitions and reads the extracted villager timetable. It does
//! not implement a general `GoalSelector` or `Brain`, and it owns no authoritative entity state.

#![forbid(unsafe_code)]

pub(crate) mod goal_policy;
pub(crate) mod schedule;

#[cfg(test)]
mod goal_policy_tests;
#[cfg(test)]
mod schedule_tests;
