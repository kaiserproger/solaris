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
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&blocks_report)
            .context("building block-state registry from blocks.json")?,
    );
    tracing::info!(
        blocks = blocks_report.len(),
        states = block_states,
        path = %blocks_path.display(),
        "block registry source loaded",
    );

    let world: Option<mc_net::WorldHandle> = if let Some(world_dir) = &cfg.data.world_dir {
        let open_result = (|| -> Result<mc_world::WorldStorage> {
            ensure_world_region_root(world_dir)?;
            Ok(mc_world::WorldStorage::open_with_capacities(
                world_dir,
                Arc::clone(&blocks),
                chunk_cache_size_for_view_distance(cfg.server.view_distance),
                cfg.chunk_pipeline.region_cache_size,
            )?)
        })();
        match open_result {
            Ok(storage) => {
                let region_count = count_region_files(world_dir);
                // M7: attach the terrain generator. Chunks missing
                // from disk get materialised on demand; the M6 flush
                // path then persists them so this only runs once per
                // chunk per fresh world.
                let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(
                    mc_worldgen::TerrainGenerator::new(cfg.data.seed, Arc::clone(&blocks)),
                );
                let storage = storage.with_generator(generator);
                tracing::info!(
                    path = %world_dir.display(),
                    block_count = storage.registry().len(),
                    region_files = region_count,
                    seed = cfg.data.seed,
                    "world storage opened with worldgen baseline",
                );
                Some(Arc::new(tokio::sync::Mutex::new(storage)))
            }
            Err(err) => {
                tracing::warn!(
                    path = %world_dir.display(),
                    error = %err,
                    "world directory not usable; starting without world (chunk queries will return None)",
                );
                None
            }
        }
    } else {
        tracing::warn!(
            "no [data].world_dir configured; chunks will not stream until one is wired up",
        );
        None
    };

    let data = Arc::new(data);
    let tags = match mc_data::tags::load(&cfg.data.vanilla_dir, &data) {
        Ok(t) => {
            tracing::info!(
                registries = t.registries.len(),
                tags = t.total_tags(),
                entries = t.total_entries(),
                path = %cfg.data.vanilla_dir.display(),
                "tag set loaded",
            );
            Arc::new(t)
        }
        Err(err) => {
            tracing::warn!(
                path = %cfg.data.vanilla_dir.display(),
                error = %err,
                "tag set load failed; the configuration handler will ship an empty Update Tags \
                 packet and the vanilla client will reject login at registry freeze",
            );
            Arc::new(mc_data::tags::TagsData::default())
        }
    };

    let block_light_path = cfg
        .data
        .vanilla_dir
        .join("reports")
        .join("block_light.json");
    let block_light = match mc_data::block_light::load(&block_light_path) {
        Ok(table) => {
            tracing::info!(
                version = %table.version,
                states = table.len(),
                path = %block_light_path.display(),
                "block-light table loaded",
            );
            Some(Arc::new(table))
        }
        Err(err) => {
            tracing::warn!(
                path = %block_light_path.display(),
                error = %err,
                "block-light table load failed; chunk streaming will keep emitting \
                 LightData::empty() until tools/extract-block-light.sh is run",
            );
            None
        }
    };

    let items_path = cfg.data.vanilla_dir.join("reports").join("registries.json");
    let items = match mc_data::items::load_items_report(&items_path) {
        Ok(report) => {
            let reg = mc_data::items::ItemRegistry::from_report(&report);
            tracing::info!(
                entries = reg.len(),
                path = %items_path.display(),
                "item registry loaded",
            );
            Arc::new(reg)
        }
        Err(err) => {
            tracing::warn!(
                path = %items_path.display(),
                error = %err,
                "item registry load failed; M6 place will fall back to stone",
            );
            Arc::new(mc_data::items::ItemRegistry::default())
        }
    };

    let world_for_shutdown = net_world_clone(&world);
    let net = cfg
        .to_network(data, blocks, world, tags, block_light, items)
        .with_context(|| format!("translating bind_address from {}", path.display()))?;
    tracing::info!(
        version = mc_server::VERSION,
        protocol = mc_protocol::PROTOCOL_VERSION,
        target = mc_protocol::TARGET_RELEASE,
        "Solaris starting",
    );

    // M6.b: race the network listener against a Ctrl-C signal. On
    // signal, stop the listener, take the world mutex exclusively,
    // and flush every dirty chunk back to disk before returning.
    let run_fut = mc_net::run(net);
    let shutdown = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %err, "ctrl_c handler failed; running without graceful shutdown");
            // Never resolve — let the listener own the lifetime.
            std::future::pending::<()>().await;
        }
    };
    tokio::select! {
        result = run_fut => {
            result.context("network listener")
        }
        () = shutdown => {
            tracing::info!("shutdown signal received");
            if let Some(world) = world_for_shutdown {
                let mut guard = world.lock().await;
                match guard.flush_dirty() {
                    Ok(n) => tracing::info!(flushed = n, "shutdown: flushed dirty chunks"),
                    Err(err) => tracing::error!(error = %err, "shutdown: flush_dirty failed"),
                }
            }
            Ok(())
        }
    }
}

fn net_world_clone(world: &Option<mc_net::WorldHandle>) -> Option<mc_net::WorldHandle> {
    world.as_ref().map(Arc::clone)
}

fn chunk_cache_size_for_view_distance(view_distance: i32) -> usize {
    let width = view_distance.max(0) as usize * 2 + 3;
    width * width
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

fn ensure_world_region_root(world_dir: &Path) -> Result<()> {
    let modern = world_dir
        .join("dimensions")
        .join("minecraft")
        .join("overworld")
        .join("region");
    let legacy = world_dir.join("region");
    if modern.is_dir() || legacy.is_dir() {
        return Ok(());
    }
    std::fs::create_dir_all(&legacy)
        .with_context(|| format!("creating empty world region directory {}", legacy.display()))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_world_region_root_creates_legacy_layout_for_missing_world() {
        let tmp = tempfile::tempdir().unwrap();
        let world = tmp.path().join("new-world");

        ensure_world_region_root(&world).unwrap();

        assert!(world.join("region").is_dir());
    }

    #[test]
    fn ensure_world_region_root_keeps_existing_modern_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let modern = tmp
            .path()
            .join("dimensions")
            .join("minecraft")
            .join("overworld")
            .join("region");
        std::fs::create_dir_all(&modern).unwrap();

        ensure_world_region_root(tmp.path()).unwrap();

        assert!(modern.is_dir());
        assert!(!tmp.path().join("region").exists());
    }

    #[test]
    fn chunk_cache_size_covers_view_plus_light_border() {
        assert_eq!(chunk_cache_size_for_view_distance(0), 9);
        assert_eq!(chunk_cache_size_for_view_distance(10), 529);
        assert_eq!(chunk_cache_size_for_view_distance(-1), 9);
    }
}
