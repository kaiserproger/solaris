//! `/tick query|rate|step|sprint|freeze|unfreeze` (vanilla TickCommand.java).
//!
//! Permission is vanilla `LEVEL_ADMINS` (op level 3); the edge enforces it from
//! `command_tree::ROOTS` before the line reaches the tick.

use mc_nbt::Tag;

use crate::boundary::{ConnectionId, PlayerChange};
use crate::command_feedback::{text, translated};
use crate::commands::CommandError;
use crate::tick::TickState;
use crate::tick_control::{announce_state, announce_step};

const MAX_RATE: f32 = 10_000.0;

#[derive(Debug, Clone, PartialEq)]
pub enum TickCommand {
    Query,
    Rate(f32),
    /// `step [<time>]`: frozen ticks to run (default 1).
    Step(i32),
    StepStop,
    Sprint(i32),
    SprintStop,
    Freeze,
    Unfreeze,
}

/// `TimeArgument.parse`: a float with optional `d`/`s`/`t` unit, rounded to
/// ticks, bounded below by `min` (`argument.time.*` errors).
fn time_argument(word: &str, min: i32) -> Result<i32, CommandError> {
    let (number, scale) = match word.as_bytes().last() {
        Some(b'd') => (&word[..word.len() - 1], 24_000.0_f32),
        Some(b's') => (&word[..word.len() - 1], 20.0),
        Some(b't') => (&word[..word.len() - 1], 1.0),
        Some(b'0'..=b'9' | b'.') => (word, 1.0),
        _ => {
            return Err(CommandError::Message(
                "Invalid time unit (expected d, s or t)".into(),
            ));
        }
    };
    let value: f32 = number
        .parse()
        .ok()
        .filter(|value: &f32| value.is_finite())
        .ok_or_else(|| CommandError::Message(format!("Invalid float '{number}'")))?;
    let ticks = (value * scale).round();
    if ticks < min as f32 {
        return Err(CommandError::Message(format!(
            "Tick count must not be less than {min}, found {}",
            ticks as i64
        )));
    }
    if ticks > i32::MAX as f32 {
        return Err(CommandError::Message(format!("Invalid integer '{word}'")));
    }
    Ok(ticks as i32)
}

pub(crate) fn parse(args: &[&str]) -> Result<TickCommand, CommandError> {
    const USAGE: &str = "tick (query|rate|step|sprint|unfreeze|freeze)";
    match args {
        ["query"] => Ok(TickCommand::Query),
        ["rate", rate] => {
            let value: f32 = rate
                .parse()
                .ok()
                .filter(|value: &f32| value.is_finite())
                .ok_or_else(|| CommandError::Message(format!("Invalid float '{rate}'")))?;
            if value < 1.0 {
                return Err(CommandError::Message(format!(
                    "Float must not be less than 1.0, found {value:?}"
                )));
            }
            if value > MAX_RATE {
                return Err(CommandError::Message(format!(
                    "Float must not be more than 10000.0, found {value:?}"
                )));
            }
            Ok(TickCommand::Rate(value))
        }
        ["step"] => Ok(TickCommand::Step(1)),
        ["step", "stop"] => Ok(TickCommand::StepStop),
        ["step", time] => Ok(TickCommand::Step(time_argument(time, 1)?)),
        ["sprint", "stop"] => Ok(TickCommand::SprintStop),
        ["sprint", time] => Ok(TickCommand::Sprint(time_argument(time, 1)?)),
        ["unfreeze"] => Ok(TickCommand::Unfreeze),
        ["freeze"] => Ok(TickCommand::Freeze),
        _ => Err(CommandError::Usage(USAGE)),
    }
}

fn success(state: &mut TickState, connection: ConnectionId, key: &str, arguments: Vec<Tag>) {
    crate::command_execution::feedback(state, connection, key, arguments);
}

/// `CommandSourceStack.sendFailure`: red translated component.
fn failure(state: &mut TickState, connection: ConnectionId, key: &str) {
    let Tag::Compound(mut fields) = translated(key, Vec::new()) else {
        unreachable!("translated builds a compound");
    };
    fields.push(("color".into(), Tag::String("red".into())));
    state.push_player(
        connection,
        PlayerChange::StyledChat {
            content: Tag::Compound(fields),
        },
    );
}

pub(crate) fn execute(state: &mut TickState, connection: ConnectionId, command: &TickCommand) {
    match command {
        TickCommand::Query => query(state, connection),
        TickCommand::Rate(rate) => {
            state.tick_control.set_rate(*rate);
            announce_state(state);
            success(
                state,
                connection,
                "commands.tick.rate.success",
                vec![text(format!("{rate:.1}"))],
            );
        }
        TickCommand::Step(ticks) => {
            if state.tick_control.step(*ticks) {
                announce_step(state);
                success(
                    state,
                    connection,
                    "commands.tick.step.success",
                    vec![text(ticks)],
                );
            } else {
                failure(state, connection, "commands.tick.step.fail");
            }
        }
        TickCommand::StepStop => {
            if state.tick_control.stop_stepping() {
                announce_step(state);
                success(
                    state,
                    connection,
                    "commands.tick.step.stop.success",
                    Vec::new(),
                );
            } else {
                failure(state, connection, "commands.tick.step.stop.fail");
            }
        }
        TickCommand::Sprint(ticks) => {
            let interrupted = state.tick_control.request_sprint(*ticks, connection);
            announce_state(state);
            if interrupted {
                success(
                    state,
                    connection,
                    "commands.tick.sprint.stop.success",
                    Vec::new(),
                );
            }
            success(
                state,
                connection,
                "commands.tick.status.sprinting",
                Vec::new(),
            );
        }
        TickCommand::SprintStop => match state.tick_control.stop_sprint() {
            Some(report) => {
                announce_state(state);
                success(
                    state,
                    report.requester,
                    "commands.tick.sprint.report",
                    vec![text(report.ticks_per_second), text(&report.millis_per_tick)],
                );
                success(
                    state,
                    connection,
                    "commands.tick.sprint.stop.success",
                    Vec::new(),
                );
            }
            None => failure(state, connection, "commands.tick.sprint.stop.fail"),
        },
        TickCommand::Freeze => {
            if state.tick_control.sprinting()
                && let Some(report) = state.tick_control.stop_sprint()
            {
                success(
                    state,
                    report.requester,
                    "commands.tick.sprint.report",
                    vec![text(report.ticks_per_second), text(&report.millis_per_tick)],
                );
            }
            if state.tick_control.stop_stepping() {
                announce_step(state);
            }
            state.tick_control.set_frozen(true);
            announce_state(state);
            success(state, connection, "commands.tick.status.frozen", Vec::new());
        }
        TickCommand::Unfreeze => {
            state.tick_control.set_frozen(false);
            announce_state(state);
            success(
                state,
                connection,
                "commands.tick.status.running",
                Vec::new(),
            );
        }
    }
}

fn query(state: &mut TickState, connection: ConnectionId) {
    let control = &state.tick_control;
    let rate = format!("{:.1}", control.rate());
    let busy = format!("{:.1}", control.average_millis());
    let (percentiles, count) = control.percentiles();
    let mut lines: Vec<(&str, Vec<Tag>)> = Vec::new();
    if control.sprinting() {
        lines.push(("commands.tick.status.sprinting", Vec::new()));
        lines.push((
            "commands.tick.query.rate.sprinting",
            vec![text(&rate), text(&busy)],
        ));
    } else {
        let status = if control.frozen() {
            "commands.tick.status.frozen"
        } else if control.target_millis() < control.average_millis() {
            "commands.tick.status.lagging"
        } else {
            "commands.tick.status.running"
        };
        lines.push((status, Vec::new()));
        lines.push((
            "commands.tick.query.rate.running",
            vec![
                text(&rate),
                text(&busy),
                text(format!("{:.1}", control.target_millis())),
            ],
        ));
    }
    lines.push((
        "commands.tick.query.percentiles",
        vec![
            text(format!("{:.1}", percentiles[0])),
            text(format!("{:.1}", percentiles[1])),
            text(format!("{:.1}", percentiles[2])),
            text(count),
        ],
    ));
    for (key, arguments) in lines {
        success(state, connection, key, arguments);
    }
}

#[cfg(test)]
#[path = "command_tick_tests.rs"]
mod tests;
