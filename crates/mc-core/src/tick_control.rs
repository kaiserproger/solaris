//! Vanilla `TickRateManager`/`ServerTickRateManager` over the native tick loop.
//!
//! Source: net/minecraft/world/TickRateManager.java, net/minecraft/server/
//! ServerTickRateManager.java (26.1.2). The manager is one global authority
//! (moved between dimension worlds by `dimensions_tick::swap_globals`) and is
//! session state: like vanilla it is never persisted.
//!
//! * `rate` stretches or shrinks the tick period (`1e9 / rate` ns); the global
//!   tick number, WAL batches and every tick-keyed clock keep advancing one per
//!   tick, so a faster rate is simply faster simulated time.
//! * `frozen` ticks (no pending `step`) still run the input, chunk, merge and
//!   commit pipeline, so players move, build and chat, but gate autonomous
//!   simulation through [`TickControl::runs_normally`]: non-player entities,
//!   random/scheduled block updates, spawning, time and weather. Tick-keyed
//!   absolute deadlines (mob timers, scheduled block ticks) are *not* paused: the
//!   WAL tick number must stay contiguous, so they fire on resume.
//! * `sprint` skips the inter-tick wait and reports the achieved rate.

use std::time::Duration;

use crate::boundary::{ConnectionId, PlayerChange};
use crate::command_feedback::text;
use crate::tick::TickState;

/// Vanilla `TickRateManager.tickrate` default.
pub const DEFAULT_RATE: f32 = 20.0;
/// Number of recent tick durations kept for `/tick query` (vanilla keeps 100).
const SAMPLES: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Sprint {
    scheduled: u64,
    remaining: u64,
    spent_us: u64,
    previous_frozen: bool,
    requester: ConnectionId,
}

/// Result of a sprint that just ended (`ServerTickRateManager.finishTickSprint`).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SprintReport {
    pub ticks_per_second: i32,
    pub millis_per_tick: String,
    pub requester: ConnectionId,
}

#[derive(Debug, Clone)]
pub(crate) struct TickControl {
    rate: f32,
    frozen: bool,
    steps: i32,
    runs_normally: bool,
    /// Ticks that did not simulate (frozen, unstepped): vanilla game time and
    /// every tick-driven clock stand still for them.
    held_ticks: u64,
    sprint: Option<Sprint>,
    samples: [u32; SAMPLES],
    cursor: usize,
    filled: usize,
}

impl Default for TickControl {
    fn default() -> Self {
        Self {
            rate: DEFAULT_RATE,
            frozen: false,
            steps: 0,
            runs_normally: true,
            held_ticks: 0,
            sprint: None,
            samples: [0; SAMPLES],
            cursor: 0,
            filled: 0,
        }
    }
}

impl TickControl {
    pub(crate) fn rate(&self) -> f32 {
        self.rate
    }

    pub(crate) fn frozen(&self) -> bool {
        self.frozen
    }

    pub(crate) fn steps(&self) -> i32 {
        self.steps
    }

    pub(crate) fn sprinting(&self) -> bool {
        self.sprint.is_some()
    }

    /// Whether this tick simulates the autonomous world (decided by
    /// [`Self::begin_tick`]; `true` before the first decision).
    pub(crate) fn runs_normally(&self) -> bool {
        self.runs_normally
    }

    /// Target wall time between ticks (`nanosecondsPerTick`).
    pub(crate) fn period(&self) -> Duration {
        Duration::from_nanos((1_000_000_000_f64 / f64::from(self.rate)) as u64)
    }

    /// Target milliseconds per tick, as vanilla prints it.
    pub(crate) fn target_millis(&self) -> f32 {
        (1_000.0_f64 / f64::from(self.rate)) as f32
    }

    pub(crate) fn set_rate(&mut self, rate: f32) {
        self.rate = rate.max(1.0);
    }

    pub(crate) fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
    }

    /// `stepGameIfPaused`: only a frozen game steps.
    pub(crate) fn step(&mut self, ticks: i32) -> bool {
        if !self.frozen {
            return false;
        }
        self.steps = ticks;
        true
    }

    pub(crate) fn stop_stepping(&mut self) -> bool {
        let stepping = self.steps > 0;
        self.steps = 0;
        stepping
    }

    /// `requestGameToSprint`: returns whether a running sprint was interrupted.
    pub(crate) fn request_sprint(&mut self, ticks: i32, requester: ConnectionId) -> bool {
        let interrupted = self.sprint.is_some();
        // A restarted sprint keeps the freeze state from before the first one.
        let previous_frozen = self
            .sprint
            .as_ref()
            .map_or(self.frozen, |sprint| sprint.previous_frozen);
        let ticks = u64::try_from(ticks).unwrap_or(0);
        self.sprint = Some(Sprint {
            scheduled: ticks,
            remaining: ticks,
            spent_us: 0,
            previous_frozen,
            requester,
        });
        self.frozen = false;
        interrupted
    }

    /// `stopSprinting`: the report of the sprint that was running, if any.
    pub(crate) fn stop_sprint(&mut self) -> Option<SprintReport> {
        self.finish_sprint()
    }

    fn finish_sprint(&mut self) -> Option<SprintReport> {
        let sprint = self.sprint.take()?;
        let completed = sprint.scheduled - sprint.remaining;
        let millis = (sprint.spent_us as f64 / 1_000.0).max(1e-6);
        let ticks_per_second = (1_000.0 * completed as f64 / millis) as i32;
        let per_tick = if completed == 0 {
            f64::from(self.target_millis())
        } else {
            millis / completed as f64
        };
        self.frozen = sprint.previous_frozen;
        Some(SprintReport {
            ticks_per_second,
            millis_per_tick: format!("{per_tick:.2}"),
            requester: sprint.requester,
        })
    }

    /// Simulated game ticks at global tick `tick` (`/time query gametime`).
    pub(crate) fn game_ticks(&self, tick: u64) -> u64 {
        tick.saturating_sub(self.held_ticks)
    }

    /// `TickRateManager.tick` plus `checkShouldSprintThisTick`. Returns the
    /// report of a sprint that completed with the previous tick.
    pub(crate) fn begin_tick(&mut self) -> Option<SprintReport> {
        self.runs_normally = !self.frozen || self.steps > 0;
        self.held_ticks += u64::from(!self.runs_normally);
        if self.steps > 0 {
            self.steps -= 1;
        }
        let sprint = self.sprint.as_mut()?;
        if self.runs_normally && sprint.remaining > 0 {
            sprint.remaining -= 1;
            return None;
        }
        if sprint.remaining == 0 {
            return self.finish_sprint();
        }
        None
    }

    /// Record one finished tick's wall time (`TickTimes::wall_us`).
    pub(crate) fn record_tick(&mut self, wall_us: u64) {
        let micros = u32::try_from(wall_us).unwrap_or(u32::MAX);
        self.samples[self.cursor] = micros;
        self.cursor = (self.cursor + 1) % SAMPLES;
        self.filled = (self.filled + 1).min(SAMPLES);
        if let Some(sprint) = &mut self.sprint {
            sprint.spent_us = sprint.spent_us.saturating_add(wall_us);
        }
    }

    /// Mean recent tick time in milliseconds (`getAverageTickTimeNanos`).
    pub(crate) fn average_millis(&self) -> f32 {
        if self.filled == 0 {
            return 0.0;
        }
        let total: u64 = self.samples[..self.filled]
            .iter()
            .map(|&us| u64::from(us))
            .sum();
        (total as f64 / self.filled as f64 / 1_000.0) as f32
    }

    /// p50/p95/p99 of the recorded tick times in milliseconds and the sample
    /// count, with vanilla's `samples[(int)(length * q)]` indexing.
    pub(crate) fn percentiles(&self) -> ([f32; 3], usize) {
        if self.filled == 0 {
            return ([0.0; 3], 0);
        }
        let mut sorted = self.samples[..self.filled].to_vec();
        sorted.sort_unstable();
        let at = |q: f64| {
            let index = ((sorted.len() as f64 * q) as usize).min(sorted.len() - 1);
            sorted[index] as f32 / 1_000.0
        };
        (
            [
                sorted[sorted.len() / 2] as f32 / 1_000.0,
                at(0.95),
                at(0.99),
            ],
            sorted.len(),
        )
    }
}

/// Start of a global tick: decide whether it simulates, finish a completed
/// sprint (feedback to the requester, state broadcast).
pub(crate) fn begin_tick(state: &mut TickState) {
    let was_frozen = state.tick_control.frozen();
    let Some(report) = state.tick_control.begin_tick() else {
        return;
    };
    announce_state_if_changed(state, was_frozen);
    tracing::info!(
        ticks_per_second = report.ticks_per_second,
        millis_per_tick = %report.millis_per_tick,
        "tick sprint finished"
    );
    crate::command_execution::feedback(
        state,
        report.requester,
        "commands.tick.sprint.report",
        vec![text(report.ticks_per_second), text(&report.millis_per_tick)],
    );
}

fn announce_state_if_changed(state: &mut TickState, was_frozen: bool) {
    if was_frozen != state.tick_control.frozen() {
        announce_state(state);
    }
}

/// Whether the simulated day clock advances this tick.
pub(crate) fn advances_day(state: &TickState) -> bool {
    state.tick_control.runs_normally()
        && crate::command_rules::boolean(state, crate::command_rules::BoolRule::ADVANCE_TIME)
}

/// Send `ClientboundTickingState` to every connected player in every world.
pub(crate) fn announce_state(state: &mut TickState) {
    let (rate, frozen) = (state.tick_control.rate(), state.tick_control.frozen());
    let connections: Vec<_> = state.connections.all_connections().collect();
    for connection in connections {
        state.push_player(connection, PlayerChange::TickingState { rate, frozen });
    }
    if state.dimension_globals.active {
        state
            .dimension_globals
            .broadcasts
            .push(crate::dimensions::globals::Broadcast::TickingState { rate, frozen });
    }
}

/// Send `ClientboundTickingStep` to every connected player in every world.
pub(crate) fn announce_step(state: &mut TickState) {
    let steps = state.tick_control.steps();
    let connections: Vec<_> = state.connections.all_connections().collect();
    for connection in connections {
        state.push_player(connection, PlayerChange::TickingStep { steps });
    }
    if state.dimension_globals.active {
        state
            .dimension_globals
            .broadcasts
            .push(crate::dimensions::globals::Broadcast::TickingStep { steps });
    }
}

/// `ServerTickRateManager.updateJoiningPlayer`.
pub(crate) fn sync_join(state: &mut TickState, connection: ConnectionId) {
    let (rate, frozen, steps) = (
        state.tick_control.rate(),
        state.tick_control.frozen(),
        state.tick_control.steps(),
    );
    state.push_player(connection, PlayerChange::TickingState { rate, frozen });
    state.push_player(connection, PlayerChange::TickingStep { steps });
}

#[cfg(test)]
#[path = "tick_control_tests.rs"]
mod tests;
