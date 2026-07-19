use std::env;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, ensure};
use mc_test_harness::replay::{ReplayRunResult, ReplayScenarioManifest};

fn main() {
    if let Err(error) = run() {
        eprintln!("core replay validation failed: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let manifest_path = PathBuf::from(
        args.next()
            .context("usage: core-replay-validate <manifest.json> <result.json>")?,
    );
    let result_path = PathBuf::from(
        args.next()
            .context("usage: core-replay-validate <manifest.json> <result.json>")?,
    );
    ensure!(
        args.next().is_none(),
        "usage: core-replay-validate <manifest.json> <result.json>"
    );

    let manifest = ReplayScenarioManifest::from_json(
        &fs::read_to_string(&manifest_path)
            .with_context(|| format!("read {}", manifest_path.display()))?,
    )?;
    let result = ReplayRunResult::from_json(
        &fs::read_to_string(&result_path)
            .with_context(|| format!("read {}", result_path.display()))?,
    )?;
    result.validate_against(&manifest)?;
    println!(
        "CORE_REPLAY_RESULT_VALID scenario={} driver={:?} outcome={:?}",
        result.scenario_id, result.driver, result.outcome
    );
    Ok(())
}
