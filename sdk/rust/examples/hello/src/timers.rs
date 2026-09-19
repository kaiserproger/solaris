//! The timer fixture: one guest that schedules, replaces and cancels timers of
//! its own.
//!
//! A plugin schedules timers by answering with commands, so everything here is a
//! batch: `script` in `config.toml` picks the scenario, every scenario timer is
//! scheduled from a callback whose tick the test pushed, and every fire is
//! reported as a broadcast line naming the timer's id, the tick it was scheduled
//! for and the tick it fired at. A test reads those triples instead of assuming a
//! delivery order, which is what makes a deferral (a fire later than its
//! deadline) visible rather than inferred.
//!
//! This lives beside the hello root rather than inside it: the root already
//! carries the one state machine every other host test drives, and a second one
//! there would make both harder to read.

use solaris_plugin_sdk::events::TimerFired;
use solaris_plugin_sdk::{broadcast, cancel_timer, schedule_timer, Command, Event};

/// The contract's bound on one callback's batch, and therefore the most timers
/// one fill callback may schedule. The fixture stays inside it: answering more
/// would be the plugin's own malformed batch, which is a different test.
const SLOTS_PER_BATCH: usize = 32;

/// How far past the current tick the capacity scripts keep their filler timers.
/// Far enough that no test reaches them, so a filler only occupies its slot.
const FILLER_DELAY_TICKS: u64 = 1000;

/// The delay every scenario timer is scheduled with.
const DELAY_TICKS: u64 = 1;

/// A timer id no script ever schedules: cancelling it is nothing to cancel
/// rather than a refusal, and the batch around it must still be admitted.
const MISSING_TIMER: &str = "missing";

/// The filler script's id for slot `index`, zero-padded so the id order the host
/// sorts by is also the order the fixture filled the slots in.
fn filler_id(index: usize) -> String {
    format!("cap-{index:03}")
}

/// The one line a test reads for a fire. Both ticks are in it because they are
/// the two facts a plugin cannot derive: a timer due at one tick fires at
/// whichever pushed tick the host observed first, and a fire deferred by the
/// per-tick bound keeps the deadline it was scheduled with.
fn fired_line(fired: &TimerFired) -> String {
    format!(
        "{}:{}:{}",
        fired.timer_id, fired.scheduled_tick, fired.fired_tick
    )
}

/// One timer scenario. A script names what each callback answers, and the scripts
/// are the fixture's own vocabulary: a test states the scenario in `config.toml`
/// and reads the fire lines back.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Script {
    /// Schedules one timer at `init` and one per join, so a test can drive both
    /// phases a timer can be scheduled from with no other behavior in the way.
    Probe,
    /// Schedules a timer, cancels a second one, cancels an id that was never
    /// scheduled, and then replaces the first: only the replaced deadline may
    /// ever fire.
    Replace,
    /// Reschedules from a callback, so the next deadline is measured from the
    /// tick the host observed when the callback ran rather than from the
    /// deadline the first timer was given.
    Chain,
    /// Schedules two timers due on the same tick; the earlier one - by id, since
    /// the deadlines are equal - cancels the later one.
    SameTick,
    /// Schedules `count` timers due on the same tick, to drive the per-tick
    /// callback bound.
    Fanout,
    /// Fills `count` slots and then asks for one new timer id, which a full
    /// plugin may not schedule.
    CapacityNew,
    /// Fills `count` slots and then asks for a new id together with a valid one
    /// in the same batch, so a full plugin refuses the batch as a whole.
    CapacityMixed,
    /// Fills `count` slots and then replaces a filler, which a full plugin may
    /// still do.
    CapacityReplace,
    /// Fills `count` slots and then sends one malformed schedule request beside
    /// a valid one.
    MalformedEmptyId,
    MalformedLongId,
    MalformedZeroDelay,
    MalformedPastBoundDelay,
    /// Schedules three timers due on one tick and faults on the middle one, so a
    /// test can pin that a tick's earlier fires publish nothing when a later one
    /// fails.
    Trap,
    /// Schedules `count` timers due on one tick and reports every fire, so one
    /// pushed tick answers `count` commands in `count` invocations.
    Budget,
    /// Finite computation per callback detects fuel rearming between due timers.
    Fuel,
    /// Timer mutations and game commands consume the same delivery allowance.
    TimerBudget,
    /// Answers a batch whose second command cannot be admitted while the first
    /// could, so a refused batch may not leave its timer behind.
    Backpressure,
    /// Schedules a timer at each of the contract's two extremes - a 64-byte id
    /// and the longest delay it admits - and then a batch a retired guest could
    /// not answer, so the edges are proven admitted rather than assumed.
    BoundaryLimits,
}

impl Script {
    /// The script `config.toml` names. An unknown name falls back to the probe,
    /// which only schedules and reports.
    fn from_name(name: &str) -> Self {
        match name {
            "replace" => Self::Replace,
            "chain" => Self::Chain,
            "same-tick" => Self::SameTick,
            "fanout" => Self::Fanout,
            "capacity-new" => Self::CapacityNew,
            "capacity-mixed" => Self::CapacityMixed,
            "capacity-replace" => Self::CapacityReplace,
            "malformed-id-empty" => Self::MalformedEmptyId,
            "malformed-id-long" => Self::MalformedLongId,
            "malformed-delay-zero" => Self::MalformedZeroDelay,
            "malformed-delay-past-bound" => Self::MalformedPastBoundDelay,
            "trap" => Self::Trap,
            "budget" => Self::Budget,
            "timer-budget" => Self::TimerBudget,
            "fuel" => Self::Fuel,
            "backpressure" => Self::Backpressure,
            "boundary-limits" => Self::BoundaryLimits,
            _ => Self::Probe,
        }
    }

    /// Whether this script fills timer slots before it sends the request under
    /// test.
    fn fills_slots(self) -> bool {
        matches!(
            self,
            Self::CapacityNew | Self::CapacityMixed | Self::CapacityReplace
        )
    }

    /// Whether this script sends a malformed request.
    fn malformed(self) -> bool {
        matches!(
            self,
            Self::MalformedEmptyId
                | Self::MalformedLongId
                | Self::MalformedZeroDelay
                | Self::MalformedPastBoundDelay
        )
    }

    /// The one malformed request this script sends.
    fn malformed_request(self) -> Command {
        match self {
            Self::MalformedEmptyId => schedule_timer("", DELAY_TICKS),
            Self::MalformedLongId => schedule_timer(&"x".repeat(65), DELAY_TICKS),
            Self::MalformedZeroDelay => schedule_timer("bad-delay", 0),
            Self::MalformedPastBoundDelay => schedule_timer("bad-delay", 630_720_001),
            _ => unreachable!("only a malformed script asks for a malformed request"),
        }
    }
}

/// The fixture itself: the script it runs and the little state a script needs to
/// answer a different batch on each phase.
pub struct Timers {
    script: Script,
    /// Slots the capacity scripts mean to fill, from `count`.
    count: usize,
    /// Filler timers scheduled so far.
    filled: usize,
    /// Joins answered since the fill finished: the capacity scripts send the
    /// request under test on the first of them and a batch a full plugin may
    /// still make on every one after it.
    phase: u32,
    /// Joins answered so far, for the scripts whose batches differ per phase.
    joins: u32,
}

impl Timers {
    /// The fixture `script` in `config.toml` names, with the `count` its scripts
    /// use.
    #[must_use]
    pub fn new(script: &str, count: usize) -> Self {
        Self {
            script: Script::from_name(script),
            count,
            filled: 0,
            phase: 0,
            joins: 0,
        }
    }

    /// The batch `init` answers with. Only the probe schedules here, because
    /// `init` runs before any tick the test pushed: every scenario that measures
    /// a deadline is anchored to a tick the test saw the host observe.
    pub fn init(&mut self) -> Vec<Command> {
        match self.script {
            Script::Probe => vec![schedule_timer("probe-init", DELAY_TICKS)],
            _ => Vec::new(),
        }
    }

    /// The commands the guest answers one delivered batch with.
    pub fn on_events(&mut self, events: &[Event]) -> Vec<Command> {
        let mut commands = Vec::new();
        for event in events {
            match event {
                Event::TimerFired(fired) => commands.extend(self.report(fired)),
                Event::PlayerJoined(_) => commands.extend(self.on_join()),
                _ => {}
            }
        }
        commands
    }

    /// What one fire answers: the line a test reads, plus - for the scripts that
    /// act on their own fire - the timer change that fire implies.
    fn report(&mut self, fired: &TimerFired) -> Vec<Command> {
        if self.script == Script::Fuel {
            let mut work = 0_u64;
            for index in 0..50_000 {
                work = std::hint::black_box(work.wrapping_add(std::hint::black_box(index)));
            }
            std::hint::black_box(work);
        }
        let mut commands = vec![broadcast(fired_line(fired))];
        if self.script == Script::TimerBudget {
            commands.push(schedule_timer("held", FILLER_DELAY_TICKS));
        }
        match (self.script, fired.timer_id.as_str()) {
            // The next deadline is two ticks past the tick the host observed
            // when this callback ran, not two ticks past this timer's own
            // deadline: a fire the host deferred is where those two differ.
            (Script::Chain, "first") => commands.push(schedule_timer("second", 2)),
            (Script::SameTick, "a-keep") => commands.push(cancel_timer("b-cancel")),
            (Script::Trap, "b-trap") => {
                // The batch above is already built when the guest faults, which
                // is what makes this a discarded prefix rather than a missing
                // batch.
                std::hint::black_box(&commands);
                fault();
            }
            _ => {}
        }
        commands
    }

    /// What one join answers.
    fn on_join(&mut self) -> Vec<Command> {
        self.joins += 1;
        if self.script.fills_slots() {
            return self.capacity_batch();
        }
        if self.script.malformed() {
            return match self.joins {
                // The admitted batch the malformed one is contrasted with follows
                // it in the same deployment, so the only difference between the
                // two is the request under test.
                1 => vec![schedule_timer("probe-join", DELAY_TICKS)],
                2 => {
                    let mut commands = vec![schedule_timer("probe-join-2", DELAY_TICKS)];
                    commands.push(self.script.malformed_request());
                    commands
                }
                // A batch a guest that survived the refusal would answer: a test
                // reads whether it ever fires.
                _ => vec![schedule_timer("recover", DELAY_TICKS)],
            };
        }
        match self.script {
            Script::Probe => vec![schedule_timer(
                &format!("probe-join-{}", self.joins),
                DELAY_TICKS,
            )],
            Script::Replace => vec![
                schedule_timer("repeat", 5),
                schedule_timer("cancelled", 2),
                cancel_timer("cancelled"),
                cancel_timer(MISSING_TIMER),
                schedule_timer("repeat", 3),
            ],
            Script::Chain => vec![schedule_timer("first", DELAY_TICKS)],
            Script::SameTick => vec![
                schedule_timer("a-keep", DELAY_TICKS),
                schedule_timer("b-cancel", DELAY_TICKS),
            ],
            Script::Fanout => self.fan_out("timer", self.count),
            Script::Budget | Script::TimerBudget => {
                // Fill across two callbacks so both seven- and eight-command
                // delivery budgets admit the setup of eight pending timers.
                let half = self.count.div_ceil(2);
                let start = (self.joins as usize - 1) * half;
                (start..(start + half).min(self.count))
                    .map(|index| schedule_timer(&format!("t-{index:02}"), DELAY_TICKS))
                    .collect()
            }
            Script::Fuel => self.fan_out("t", self.count),
            Script::Trap => vec![
                schedule_timer("a-probe", DELAY_TICKS),
                schedule_timer("b-trap", DELAY_TICKS),
                schedule_timer("c-probe", DELAY_TICKS),
            ],
            Script::Backpressure => {
                if self.joins == 1 {
                    // Two lines the command queue cannot both hold, beside a timer
                    // the same batch asks for: the delivery is refused as a whole,
                    // so the timer must not survive the lines that were refused.
                    vec![
                        schedule_timer("held", DELAY_TICKS),
                        broadcast("mixed-1"),
                        broadcast("mixed-2"),
                    ]
                } else {
                    vec![schedule_timer("control", DELAY_TICKS)]
                }
            }
            Script::BoundaryLimits => {
                if self.joins == 1 {
                    // Both extremes in one batch: the longest id the contract's
                    // script-id bound admits, and the longest delay it admits.
                    // The batch is admitted or the guest is retired, so a test
                    // reads the edges from a later batch still being answered.
                    vec![
                        schedule_timer(&"b".repeat(64), DELAY_TICKS),
                        schedule_timer("far", 630_720_000),
                    ]
                } else {
                    vec![schedule_timer("after", DELAY_TICKS)]
                }
            }
            Script::CapacityNew
            | Script::CapacityMixed
            | Script::CapacityReplace
            | Script::MalformedEmptyId
            | Script::MalformedLongId
            | Script::MalformedZeroDelay
            | Script::MalformedPastBoundDelay => unreachable!("both are answered above"),
        }
    }

    /// The batch one join answers for a capacity script: filler batches until
    /// `count` slots are held, then the request under test, then a request a full
    /// plugin may still make, so a refusal is never the last thing a test can
    /// observe.
    fn capacity_batch(&mut self) -> Vec<Command> {
        if self.filled < self.count {
            return self.fill();
        }
        self.phase += 1;
        match (self.script, self.phase) {
            (Script::CapacityNew, 1) => vec![schedule_timer("overflow", DELAY_TICKS)],
            (Script::CapacityMixed, 1) => vec![
                schedule_timer("probe-join", DELAY_TICKS),
                schedule_timer("overflow", DELAY_TICKS),
            ],
            // Every later join replaces the first filler, and so does every join
            // of the replacement script: the one scheduling request a full
            // plugin may still make.
            (_, _) => vec![schedule_timer(&filler_id(0), DELAY_TICKS)],
        }
    }

    /// One batch of filler timers, continuing where the last batch stopped.
    fn fill(&mut self) -> Vec<Command> {
        let start = self.filled;
        let end = (start + SLOTS_PER_BATCH).min(self.count);
        self.filled = end;
        (start..end)
            .map(|index| schedule_timer(&filler_id(index), FILLER_DELAY_TICKS))
            .collect()
    }

    /// One `count`-long batch of timers due on the next tick, all named
    /// `<prefix>-<index>`.
    fn fan_out(&self, prefix: &str, count: usize) -> Vec<Command> {
        (0..count)
            .map(|index| schedule_timer(&format!("{prefix}-{index:02}"), DELAY_TICKS))
            .collect()
    }
}

/// Fault the guest with the batch it built still in hand.
fn fault() -> ! {
    core::arch::wasm32::unreachable()
}
