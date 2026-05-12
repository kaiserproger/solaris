//! `mc-server` binary entry point.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

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

    let data = mc_data::load(&cfg.data.vanilla_dir).with_context(|| {
        format!(
            "loading vanilla data sidecar from {}",
            cfg.data.vanilla_dir.display()
        )
    })?;
    tracing::info!(
        registries = data.registry_count(),
        entries = data.entry_count(),
        path = %cfg.data.vanilla_dir.display(),
        "vanilla data loaded",
    );

    let blocks_path = cfg.data.vanilla_dir.join("reports").join("blocks.json");
    let blocks_report = mc_data::blocks::load_blocks_report(&blocks_path).with_context(|| {
        format!(
            "loading blocks report from {}; run tools/extract-vanilla-data.sh \
             to regenerate the sidecar",
            blocks_path.display(),
        )
    })?;
    let block_states: usize = blocks_report.iter().map(|b| b.states.len()).sum();
    tracing::info!(
        blocks = blocks_report.len(),
        states = block_states,
        path = %blocks_path.display(),
        "block registry source loaded",
    );

    if let Some(world_dir) = &cfg.data.world_dir {
        match mc_world::WorldStorage::open(world_dir, &blocks_report) {
            Ok(world) => {
                let region_count = count_region_files(world_dir);
                tracing::info!(
                    path = %world_dir.display(),
                    block_count = world.registry().len(),
                    region_files = region_count,
                    "world storage opened",
                );
            }
            Err(err) => {
                tracing::warn!(
                    path = %world_dir.display(),
                    error = %err,
                    "world directory not usable; starting without world (chunk queries will return None)",
                );
            }
        }
    } else {
        tracing::warn!(
            "no [data].world_dir configured; chunk queries will return None until M3 wires the world into the network layer",
        );
    }

    let net = cfg
        .to_network(Arc::new(data))
        .with_context(|| format!("translating bind_address from {}", path.display()))?;
    tracing::info!(
        version = mc_server::VERSION,
        protocol = mc_protocol::PROTOCOL_VERSION,
        target = mc_protocol::TARGET_RELEASE,
        "Solaris starting",
    );
    mc_net::run(net).await.context("network listener")
}

fn count_region_files(world_dir: &Path) -> usize {
    let mut total = 0;
    for candidate in [
        world_dir.join("dimensions/minecraft/overworld/region"),
        world_dir.join("region"),
    ] {
        if let Ok(entries) = std::fs::read_dir(&candidate) {
            for e in entries.flatten() {
                if e.path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("mca"))
                {
                    total += 1;
                }
            }
        }
    }
    total
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
