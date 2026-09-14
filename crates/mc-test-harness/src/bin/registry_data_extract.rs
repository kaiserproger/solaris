//! Thin CLI over [`mc_test_harness::registry_capture`].
//!
//! The capture step itself lives in the library so the packaged server importer
//! can run it without a repository checkout; this binary keeps the developer
//! workflow (`tools/extract-vanilla-data.sh` step 8) unchanged.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use mc_protocol::{PROTOCOL_VERSION, TARGET_RELEASE};
use mc_test_harness::registry_capture::capture_registry_payloads_from_jar;

#[derive(Debug, Parser)]
#[command(
    name = "registry-data-extract",
    about = "Capture exact full RegistryData payloads from a local vanilla server"
)]
struct Cli {
    /// Mojang server bundle jar for the exact Solaris target release.
    #[arg(long, default_value = ".analysis/server.jar")]
    jar: PathBuf,

    /// Existing vanilla sidecar populated by tools/extract-vanilla-data.sh.
    #[arg(long, default_value = "data/vanilla")]
    out: PathBuf,

    /// Maximum time to wait for the vanilla process to announce readiness.
    #[arg(long, default_value_t = 90)]
    startup_timeout_seconds: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let java = std::env::var_os("JAVA").map_or_else(|| PathBuf::from("java"), PathBuf::from);
    let scratch = tempfile::tempdir().context("create the capture scratch directory")?;

    let captured = capture_registry_payloads_from_jar(
        &cli.jar,
        scratch.path(),
        &cli.out,
        &java,
        Duration::from_secs(cli.startup_timeout_seconds),
    )
    .await?;

    let entry_count = captured
        .values()
        .map(std::collections::BTreeSet::len)
        .sum::<usize>();
    println!(
        "captured exact RegistryData fallback for {TARGET_RELEASE} protocol {PROTOCOL_VERSION}: {} registries, {entry_count} entries",
        captured.len()
    );
    Ok(())
}
