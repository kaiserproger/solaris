use std::future::Future;

use anyhow::Result;
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};

use crate::OperatorCommand;

#[derive(Debug, Parser)]
#[command(
    name = "console",
    no_binary_name = true,
    disable_help_subcommand = true
)]
struct CommandLine {
    #[command(subcommand)]
    command: ConsoleCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum ConsoleCommand {
    /// Show available console commands.
    Help,
    /// Show current server performance and world seed.
    Status,
    /// Write measured tick phases, memory and population counters to logs/profile.json.
    Profile,
    /// List connected players.
    List,
    /// List loaded plugins.
    Plugins,
    /// Save admitted world and player state.
    #[command(name = "save-all")]
    Save,
    /// Drain, save and stop the server.
    Stop,
    /// Set the world clock through the simulation owner.
    Time {
        #[command(subcommand)]
        command: TimeCommand,
    },
    /// Change current weather.
    Weather { kind: Weather },
    /// Read or change supported world rules.
    Gamerule {
        #[command(subcommand)]
        command: GameRule,
    },
    /// Manage persisted operators (effective at next server start).
    Operator {
        #[command(subcommand)]
        command: OperatorCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum TimeCommand {
    Set {
        #[arg(value_parser = parse_time)]
        value: u64,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum Weather {
    Clear,
    Rain,
    Thunder,
}

#[derive(Debug, Subcommand)]
pub(crate) enum GameRule {
    #[command(name = "doDaylightCycle")]
    DaylightCycle { value: Option<bool> },
    #[command(name = "playersSleepingPercentage")]
    PlayersSleepingPercentage { value: Option<u32> },
}

#[derive(Clone, ValueEnum)]
enum TimeOfDay {
    Day,
    Noon,
    Night,
    Midnight,
}

fn parse_time(value: &str) -> Result<u64, String> {
    if let Ok(ticks) = value.parse::<u64>() {
        return Ok(ticks);
    }
    Ok(match TimeOfDay::from_str(value, false)? {
        TimeOfDay::Day => 1_000,
        TimeOfDay::Noon => 6_000,
        TimeOfDay::Night => 13_000,
        TimeOfDay::Midnight => 18_000,
    })
}

impl ConsoleCommand {
    pub fn parse(input: &str) -> Result<Self, clap::Error> {
        CommandLine::try_parse_from(input.split_whitespace()).map(|line| line.command)
    }

    pub fn help() -> String {
        CommandLine::command().render_long_help().to_string()
    }
}

pub(crate) enum ConsoleReply {
    Output(String),
    Shutdown,
}

/// Typed control boundary. Rendering and input editing do not own server policy.
pub(crate) trait CommandHandler {
    fn execute(&self, command: ConsoleCommand) -> impl Future<Output = Result<ConsoleReply>>;
}
