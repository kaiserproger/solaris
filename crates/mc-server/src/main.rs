//! `mc-server` binary entry point.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use mc_server::ServerConfig;

#[derive(Debug, Parser)]
#[command(
    name = "mc-server",
    version,
    about = "Solaris Minecraft-compatible server"
)]
struct Cli {
    /// Path to the server configuration file (TOML).
    #[arg(long, default_value = "config.toml")]
    config: PathBuf,

    /// Parse the configuration file, print it as JSON, and exit without
    /// starting the network listener. Useful for CI sanity checks.
    #[arg(long)]
    check: bool,
}

fn load_config(path: &Path) -> Result<ServerConfig> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading config file {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parsing config file {}", path.display()))
}

fn check_config(path: &Path) -> Result<()> {
    let cfg = load_config(path)?;
    let rendered = serde_json::to_string_pretty(&cfg).context("rendering config as JSON")?;
    println!("{rendered}");
    Ok(())
}

async fn serve(path: &Path) -> Result<()> {
    let cfg = load_config(path)?;
    let net = cfg
        .to_network()
        .with_context(|| format!("translating bind_address from {}", path.display()))?;
    tracing::info!(
        version = mc_server::VERSION,
        protocol = mc_protocol::PROTOCOL_VERSION,
        target = mc_protocol::TARGET_RELEASE,
        "Solaris starting",
    );
    mc_net::run(net).await.context("network listener")
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
}

#[tokio::main]
async fn main() -> ExitCode {
    init_tracing();
    let cli = Cli::parse();
    let result = if cli.check {
        check_config(&cli.config)
    } else {
        serve(&cli.config).await
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}
