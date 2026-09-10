use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use mc_server::{OperatorFileOperation, dashboard::DashboardStats};

use super::commands::{
    CommandHandler, ConsoleCommand, ConsoleReply, GameRule, TimeCommand, Weather,
};
use crate::OperatorCommand;

pub(crate) struct ServerCommands {
    pub stats: Arc<dyn DashboardStats>,
    pub save: mc_net::SaveHandle,
    pub control: mc_net::OperatorControlHandle,
    pub config_path: PathBuf,
}

impl CommandHandler for ServerCommands {
    async fn execute(&self, command: ConsoleCommand) -> Result<ConsoleReply> {
        let message = match command {
            ConsoleCommand::Help => ConsoleCommand::help(),
            ConsoleCommand::Profile => {
                let snapshot = self.stats.stats();
                tokio::fs::create_dir_all("logs").await?;
                let report = serde_json::to_vec_pretty(&snapshot)?;
                tokio::fs::write("logs/profile.json", report).await?;
                format!(
                    "Profile saved: logs/profile.json ({} tick samples, p95 {} us, memory {} MiB)",
                    snapshot.tick.total.samples,
                    snapshot.tick.total.p95_us,
                    snapshot.memory.used_mb,
                )
            }
            ConsoleCommand::Status => {
                let stats = self.stats.stats();
                format!(
                    "{} players, {:.1} TPS, p95 {:.2} ms, {} MiB; seed {}",
                    stats.players.count,
                    stats.tps,
                    stats.tick.total.p95_us as f64 / 1000.0,
                    stats.memory.used_mb,
                    stats.world.seed
                )
            }
            ConsoleCommand::List => {
                let stats = self.stats.stats();
                format!(
                    "Online ({}/{}): {}",
                    stats.players.count,
                    stats.players.max,
                    stats.players.names.join(", ")
                )
            }
            ConsoleCommand::Plugins => {
                format!("Loaded: {}", self.stats.stats().plugins.loaded.join(", "))
            }
            ConsoleCommand::Save => {
                let report = self.save.save_all().await;
                if !report.is_ok() {
                    anyhow::bail!("save failed: {:?}", report.errors);
                }
                "Save completed successfully.".into()
            }
            ConsoleCommand::Stop => {
                self.control.request_stop();
                return Ok(ConsoleReply::Shutdown);
            }
            ConsoleCommand::Time {
                command: TimeCommand::Set { value },
            } => {
                self.control.set_world_time(value).await?;
                format!("World time set to {value}.")
            }
            ConsoleCommand::Weather { kind } => {
                self.control.set_weather(match kind {
                    Weather::Clear => mc_net::OperatorWeather::Clear,
                    Weather::Rain => mc_net::OperatorWeather::Rain,
                    Weather::Thunder => mc_net::OperatorWeather::Thunder,
                });
                format!("Weather set to {kind:?}.")
            }
            ConsoleCommand::Gamerule { command } => match command {
                GameRule::DaylightCycle { value } => {
                    format!("doDaylightCycle = {}", self.control.daylight_cycle(value))
                }
                GameRule::PlayersSleepingPercentage { value } => format!(
                    "playersSleepingPercentage = {}",
                    self.control.players_sleeping_percentage(value)
                ),
            },
            ConsoleCommand::Operator { command } => {
                let operation = match command {
                    OperatorCommand::Add { identity } => OperatorFileOperation::Add(identity),
                    OperatorCommand::Remove { identity } => OperatorFileOperation::Remove(identity),
                    OperatorCommand::List => OperatorFileOperation::List,
                };
                let path = self.config_path.clone();
                let report = tokio::task::spawn_blocking(move || -> Result<_> {
                    let mut config = crate::load_config(&path)?;
                    if config.admin.operators_file.is_none() {
                        config.admin.operators_file = Some(PathBuf::from("ops.json"));
                    }
                    config.manage_operator_file(&path, operation)
                })
                .await??;
                format!(
                    "Persisted operators: {}. Changes take effect at next server start.",
                    report.identities.join(", ")
                )
            }
        };
        Ok(ConsoleReply::Output(message))
    }
}
