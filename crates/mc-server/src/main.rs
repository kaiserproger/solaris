//! `mc-server` binary entry point.
//!
//! In M0 this is intentionally tiny: it parses CLI arguments, loads a TOML
//! configuration, and pretty-prints it. Real server runtime (tokio, tick loop,
//! networking) is introduced in M1+.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use mc_server::ServerConfig;

#[derive(Debug, Parser)]
#[command(
    name = "mc-server",
    version,
    about = "Solaris Minecraft-compatible server (M0 scaffold)"
)]
struct Cli {
    /// Path to the server configuration file (TOML).
    #[arg(long, default_value = "config.toml")]
    config: PathBuf,
}

fn run(cli: Cli) -> Result<()> {
    let raw = std::fs::read_to_string(&cli.config)
        .with_context(|| format!("reading config file {}", cli.config.display()))?;
    let cfg: ServerConfig = toml::from_str(&raw)
        .with_context(|| format!("parsing config file {}", cli.config.display()))?;
    let rendered = serde_json::to_string_pretty(&cfg).context("rendering config as JSON")?;
    println!("{rendered}");
    Ok(())
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}
