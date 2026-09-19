//! The timers one plugin owns, and the bounds the contract states about them.
//!
//! A timer is host-side memory, not a server object: the host that owns an
//! instance keeps its schedule, fires it from the simulation ticks the server
//! already pushes into the boundary, and drops it when the instance is retired.
//! the bounds a request is checked against - and nothing here calls a guest, a
//! server or a clock. The schedule has one component-host convention: deadline
//! then timer id, a bounded pending count, and a bounded number of callbacks per
//! tick.

use std::collections::{BTreeSet, HashMap};

use crate::bindings::solaris::plugin::commands::Command;

/// Timers one plugin may hold at once.
///
/// The bound belongs to the instance, not to the call that filled it: a plugin
/// that keeps every timer it ever scheduled is refused the one past this, while a
/// plugin that keeps a few ids alive may replace them forever.
pub const MAX_PENDING_TIMERS: usize = 256;

/// Timer callbacks one pushed simulation tick fires.
///
/// The rest of the due timers wait for a later pushed tick: a plugin cannot make
/// one tick an unbounded amount of host work by making many timers due at once,
/// and a deferred timer keeps the deadline it was scheduled for.
pub const MAX_TIMER_CALLBACKS_PER_TICK: usize = 8;

/// Longest delay one schedule may ask for, in ticks.
///
/// The delay is relative to the tick the callback observed and the sum is
/// checked, so this bound is a policy about what a plugin may ask for, not a
/// limit of the tick space itself.
pub const MAX_TIMER_DELAY_TICKS: u64 = 630_720_000;

/// One plugin's own timers: every id it holds with the deadline it waits for.
///
/// The two maps are the same set seen twice. The deadline index is keyed by
/// `(deadline, timer_id)`, so it both orders the timers the contract's way -
/// earliest deadline first, then smallest id - and says which id a taken deadline
/// belonged to.
#[derive(Clone, Debug, Default)]
pub struct TimerSchedule {
    by_id: HashMap<String, u64>,
    by_deadline: BTreeSet<(u64, String)>,
}

impl TimerSchedule {
    /// Schedule `timer_id` for `deadline`, replacing the timer of that id.
    ///
    /// A replace is admitted while the plugin holds the maximum on purpose: an id
    /// it already owns is not one timer too many, and a plugin that reschedules
    /// one timer every tick must never be refused for holding too many. Only
    /// naming an id the plugin does not hold while [`MAX_PENDING_TIMERS`] are held
    /// is refused.
    pub fn schedule(&mut self, timer_id: String, deadline: u64) -> Result<(), TimerRefusal> {
        if !self.by_id.contains_key(&timer_id) && self.by_id.len() >= MAX_PENDING_TIMERS {
            return Err(TimerRefusal::TooManyPending);
        }
        if let Some(previous) = self.by_id.insert(timer_id.clone(), deadline) {
            self.by_deadline.remove(&(previous, timer_id.clone()));
        }
        self.by_deadline.insert((deadline, timer_id));
        Ok(())
    }

    /// Cancel `timer_id`, and report whether the plugin held it.
    ///
    /// Cancelling a timer the plugin does not hold is not a refusal: it may
    /// already have fired, or never have been scheduled, and both are nothing to
    /// change.
    pub fn cancel(&mut self, timer_id: &str) -> bool {
        let Some(deadline) = self.by_id.remove(timer_id) else {
            return false;
        };
        self.by_deadline.remove(&(deadline, timer_id.to_owned()));
        true
    }

    /// The next timer due at `tick`, without taking it: the earliest deadline
    /// first, then the smallest timer id, and nothing at all while every deadline
    /// is still ahead.
    #[must_use]
    pub fn next_due(&self, tick: u64) -> Option<(u64, &str)> {
        let (deadline, timer_id) = self.by_deadline.first()?;
        if *deadline > tick {
            return None;
        }
        Some((*deadline, timer_id))
    }

    /// The next timer due at `tick`, taken out of the schedule so it cannot fire
    /// twice.
    pub fn take_next_due(&mut self, tick: u64) -> Option<(u64, String)> {
        self.next_due(tick)?;
        let (deadline, timer_id) = self.by_deadline.pop_first()?;
        self.by_id.remove(&timer_id);
        Some((deadline, timer_id))
    }
}

/// One timer mutation a callback asked for, already checked against the
/// contract's bounds and anchored to the tick that callback observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimerMutation {
    /// Wait for `deadline`, which is the observed tick plus the requested delay,
    /// replacing whatever the same id waited for before.
    Schedule { timer_id: String, deadline: u64 },
    /// Stop waiting for `timer_id`, whether or not the plugin still holds it.
    Cancel { timer_id: String },
}

/// Why a callback's own timer request was refused.
///
/// Every variant is the plugin's own answer breaking a bound the contract states,
/// not a state the host could retry: a plugin that asks for a timer it cannot
/// have is answered like one that answers a malformed game command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TimerRefusal {
    /// The id is empty, longer than 64 bytes, or outside the contract's
    /// script-id alphabet.
    #[error("timer id is not a contract script id")]
    InvalidId,
    /// The delay is zero or past [`MAX_TIMER_DELAY_TICKS`].
    #[error("timer delay is outside the contract's bound")]
    InvalidDelay,
    /// The observed tick plus the delay does not fit the tick space, so the
    /// deadline the plugin asked for does not exist.
    #[error("timer deadline does not fit the tick space")]
    DeadlineOverflow,
    /// The plugin holds [`MAX_PENDING_TIMERS`] timers and named one more.
    #[error("the plugin already holds its maximum of pending timers")]
    TooManyPending,
}

/// Split one callback's staged answer into the commands the server owns and the
/// timer mutations the plugin's own schedule owns.
///
/// The whole answer is checked before any of it is applied: the id, the delay,
/// and the deadline the observed tick and the delay add up to. A request the
/// contract refuses therefore fails the batch as its own malformed answer, and
/// the caller applies nothing - neither the game commands nor the timer requests
/// of the same answer.
pub fn split(
    mut commands: Vec<Command>,
    observed_tick: u64,
) -> Result<(Vec<Command>, Vec<TimerMutation>), TimerRefusal> {
    let mut mutations = Vec::new();
    for command in commands.extract_if(.., |command| {
        matches!(command, Command::ScheduleTimer(_) | Command::CancelTimer(_))
    }) {
        mutations.push(classify(command, observed_tick)?);
    }
    Ok((commands, mutations))
}

/// Apply one mutation this plugin asked for to its own schedule.
///
/// A schedule is the only step that can still refuse here, and it refuses only
/// because the plugin already holds the maximum; a cancel is applied whether or
/// not the plugin still holds the id.
pub fn apply(schedule: &mut TimerSchedule, mutation: TimerMutation) -> Result<(), TimerRefusal> {
    match mutation {
        TimerMutation::Schedule { timer_id, deadline } => schedule.schedule(timer_id, deadline),
        TimerMutation::Cancel { timer_id } => {
            schedule.cancel(&timer_id);
            Ok(())
        }
    }
}

/// The schedule one callback's timer requests would make, or nothing when the
/// callback asked for no timer change.
///
/// The instance's own schedule is the one thing a refused answer must not change,
/// so the requests are applied to a copy that the caller commits only once the
/// whole answer was admitted. The copy is made only for an answer that changes
/// something: a plugin that schedules and cancels nothing - every callback of a
/// plugin that uses no timers - pays nothing here.
pub fn stage(
    schedule: &TimerSchedule,
    mutations: Vec<TimerMutation>,
) -> Result<Option<TimerSchedule>, TimerRefusal> {
    if mutations.is_empty() {
        return Ok(None);
    }
    let mut staged = schedule.clone();
    for mutation in mutations {
        apply(&mut staged, mutation)?;
    }
    Ok(Some(staged))
}

/// Check one command of a callback's answer, converting a timer request into the
/// mutation it asks for.
fn classify(command: Command, observed_tick: u64) -> Result<TimerMutation, TimerRefusal> {
    match command {
        Command::ScheduleTimer(timer) => {
            validate_id(&timer.timer_id)?;
            if !(1..=MAX_TIMER_DELAY_TICKS).contains(&timer.delay_ticks) {
                return Err(TimerRefusal::InvalidDelay);
            }
            let deadline = observed_tick
                .checked_add(timer.delay_ticks)
                .ok_or(TimerRefusal::DeadlineOverflow)?;
            Ok(TimerMutation::Schedule {
                timer_id: timer.timer_id,
                deadline,
            })
        }
        Command::CancelTimer(timer) => {
            validate_id(&timer.timer_id)?;
            Ok(TimerMutation::Cancel {
                timer_id: timer.timer_id,
            })
        }
        _ => unreachable!("split extracts only timer commands"),
    }
}

/// Check one timer id against the contract's own script-id bound: 1..64 bytes of
/// lowercase letters, digits, `_` and `-`.
fn validate_id(timer_id: &str) -> Result<(), TimerRefusal> {
    mc_script::validate_script_id_value(timer_id).map_err(|_| TimerRefusal::InvalidId)
}
