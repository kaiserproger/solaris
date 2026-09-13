use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use mc_server::{OperatorFileOperation, dashboard::DashboardStats};

use super::commands::{
    CommandHandler, ConsoleCommand, ConsoleReply, GameRule, TimeCommand, Weather, WhitelistCommand,
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
                let provider = Arc::clone(&self.stats);
                let snapshot = tokio::task::spawn_blocking(move || provider.profile()).await?;
                tokio::fs::create_dir_all("logs").await?;
                let report = serde_json::to_vec_pretty(&snapshot)?;
                tokio::fs::write("logs/profile.json.tmp", report).await?;
                tokio::fs::rename("logs/profile.json.tmp", "logs/profile.json").await?;
                let mib = |value: &serde_json::Value| {
                    value
                        .as_u64()
                        .map(|bytes| (bytes / (1024 * 1024)).to_string())
                        .unwrap_or_else(|| "unavailable".to_owned())
                };
                format!(
                    "Profile saved: logs/profile.json (RSS {} MiB, Rust live {} MiB, CPU {} cores, capture {} ms)",
                    mib(&snapshot.process["rss_bytes"]),
                    mib(&snapshot.allocations["rust_requested_live_bytes"]),
                    snapshot.cpu["average_cores_used"]
                        .as_f64()
                        .map(|v| format!("{v:.2}"))
                        .unwrap_or_else(|| "unavailable".to_owned()),
                    snapshot.capture_wall_ms,
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
                let (identity, present) = match command {
                    OperatorCommand::Add { identity } => (identity, true),
                    OperatorCommand::Remove { identity } => (identity, false),
                    OperatorCommand::List => {
                        return Ok(ConsoleReply::Output(format!(
                            "Operators: {}",
                            self.control.operators().join(", ")
                        )));
                    }
                };
                persist_access_change(&self.config_path, AccessFile::Operators, &identity, present)
                    .await?;
                format!(
                    "Operators: {} (applied now)",
                    self.control.set_operator(&identity, present).join(", ")
                )
            }
            ConsoleCommand::Whitelist { command } => {
                let (identity, present) = match command {
                    WhitelistCommand::Add { identity } => (identity, true),
                    WhitelistCommand::Remove { identity } => (identity, false),
                    WhitelistCommand::List => {
                        return Ok(ConsoleReply::Output(format!(
                            "Whitelist: {}",
                            self.control.whitelist().join(", ")
                        )));
                    }
                };
                persist_access_change(&self.config_path, AccessFile::Whitelist, &identity, present)
                    .await?;
                format!(
                    "Whitelist: {} (applies to the next login; enforcement needs auth.whitelist_enabled)",
                    self.control.set_whitelisted(&identity, present).join(", ")
                )
            }
        };
        Ok(ConsoleReply::Output(message))
    }
}

/// Which access-control file a console mutation persists to.
#[derive(Clone, Copy)]
enum AccessFile {
    Operators,
    Whitelist,
}

/// Persist one identity change before the live set is updated, so a failed
/// write never leaves the running server granting access the file lacks.
async fn persist_access_change(
    config_path: &std::path::Path,
    file: AccessFile,
    identity: &str,
    present: bool,
) -> Result<()> {
    let path = config_path.to_path_buf();
    let identity = identity.to_owned();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut config = crate::load_config(&path)?;
        match file {
            AccessFile::Operators => {
                if config.admin.operators_file.is_none() {
                    config.admin.operators_file = Some(PathBuf::from("ops.json"));
                }
                let operation = if present {
                    OperatorFileOperation::Add(identity)
                } else {
                    OperatorFileOperation::Remove(identity)
                };
                config.manage_operator_file(&path, operation)?;
            }
            AccessFile::Whitelist => {
                if config.auth.whitelist_file.is_none() {
                    config.auth.whitelist_file = Some(PathBuf::from("whitelist.json"));
                }
                if present {
                    config.add_whitelist_identity(&path, &identity)?;
                } else {
                    config.remove_whitelist_identity(&path, &identity)?;
                }
            }
        }
        Ok(())
    })
    .await?
}
