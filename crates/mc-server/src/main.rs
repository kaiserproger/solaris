//! `mc-server` binary entry point.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use std::{future::Future, pin::Pin};

use anyhow::{Context, Result};
use clap::Parser;
use mc_server::ServerConfig;

const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(6);

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
    let effective = EffectiveConfig::from(&cfg);
    let rendered = serde_json::to_string_pretty(&effective).context("rendering config as JSON")?;
    println!("{rendered}");
    Ok(())
}

#[derive(serde::Serialize)]
struct EffectiveConfig<'a> {
    #[serde(flatten)]
    config: &'a ServerConfig,
    effective_chunk_pipeline: EffectiveChunkPipeline,
}

impl<'a> From<&'a ServerConfig> for EffectiveConfig<'a> {
    fn from(config: &'a ServerConfig) -> Self {
        Self {
            config,
            effective_chunk_pipeline: EffectiveChunkPipeline::from(
                config.chunk_pipeline.to_network(),
            ),
        }
    }
}

#[derive(serde::Serialize)]
struct EffectiveChunkPipeline {
    chunk_io_threads: usize,
    chunk_worker_threads: usize,
    entity_worker_threads: usize,
}

impl From<mc_net::ChunkPipelinePolicy> for EffectiveChunkPipeline {
    fn from(policy: mc_net::ChunkPipelinePolicy) -> Self {
        Self {
            chunk_io_threads: policy.chunk_io_threads,
            chunk_worker_threads: policy.chunk_worker_threads,
            entity_worker_threads: policy.entity_worker_threads,
        }
    }
}

async fn serve(path: &Path) -> Result<()> {
    let cfg = load_config(path)?;

    let data = mc_data::solaris_required_data();
    tracing::info!(
        registries = data.registry_count(),
        entries = data.entry_count(),
        "embedded registry index loaded",
    );

    let blocks_report = mc_data::blocks::solaris_required_blocks_report();
    let block_states: usize = blocks_report.iter().map(|b| b.states.len()).sum();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&blocks_report)
            .context("building block-state registry from embedded JSON")?,
    );
    tracing::info!(
        blocks = blocks_report.len(),
        states = block_states,
        "embedded block registry loaded",
    );
    let structure_rules = mc_worldgen::StructureRules::none();
    let chunk_pipeline = cfg.chunk_pipeline.to_network();
    let terrain_generator = build_terrain_generator(
        cfg.data.seed,
        cfg.data.worldgen_mode.to_worldgen(),
        Arc::clone(&blocks),
        structure_rules,
    );
    let items = Arc::new(mc_data::items::solaris_required_items());
    tracing::info!(entries = items.len(), "embedded item registry loaded");
    let item_facts = Arc::new(mc_data::item_components::solaris_required_item_facts());
    tracing::info!(
        entries = item_facts.len(),
        "embedded item component facts loaded"
    );

    let world: Option<mc_net::WorldHandle> = if let Some(world_dir) = &cfg.data.world_dir {
        let open_result = (|| -> Result<mc_world::WorldStorage> {
            ensure_world_region_root(world_dir)?;
            Ok(mc_world::WorldStorage::open_with_capacities(
                world_dir,
                Arc::clone(&blocks),
                chunk_cache_size_for_view_distance(cfg.server.view_distance),
                chunk_pipeline.region_cache_size,
            )?)
        })();
        match open_result {
            Ok(storage) => {
                // M7: attach the terrain generator. Chunks missing
                // from disk get materialised on demand; the M6 flush
                // path then persists them so this only runs once per
                // chunk per fresh world.
                let generator: Arc<dyn mc_world::ChunkGenerator> =
                    Arc::clone(&terrain_generator) as Arc<dyn mc_world::ChunkGenerator>;
                let mut storage = storage
                    .with_generator(generator)
                    .with_item_registry(Arc::clone(&items));
                let mut region_count = count_region_files(world_dir);
                if region_count == 0 {
                    let generated = generate_spawn_window(
                        &mut storage,
                        Arc::clone(&terrain_generator) as Arc<dyn mc_world::ChunkGenerator>,
                        cfg.server.view_distance,
                        chunk_pipeline.chunk_worker_threads,
                    )?;
                    tracing::info!("Preparing world... 95% (saving generated chunks)");
                    let flushed = storage.flush_dirty()?;
                    region_count = count_region_files(world_dir);
                    tracing::info!("Preparing world... 100%");
                    tracing::info!(
                        path = %world_dir.display(),
                        chunks = generated,
                        flushed,
                        region_files = region_count,
                        "empty world pre-generated around spawn",
                    );
                }
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
    let tags = Arc::new(mc_data::tags::solaris_required_item_tags(&items));
    tracing::info!(
        tags = tags.total_tags(),
        entries = tags.total_entries(),
        "embedded tags loaded"
    );
    let recipes = Arc::new(mc_data::recipes::solaris_required_recipes());
    tracing::info!(entries = recipes.len(), "embedded recipe registry loaded");
    let loot = Arc::new(mc_data::loot::builtin().clone());
    tracing::info!(drops = loot.total_drops(), "embedded loot tables loaded");

    let block_light = Arc::new(
        mc_data::block_light::BlockLightTable::conservative_from_blocks_report(&blocks_report),
    );
    tracing::info!(
        version = %block_light.version,
        states = block_light.len(),
        "block-light table built from blocks report",
    );

    let block_facts = Arc::new(mc_data::block_facts::BlockFactsTable::from_blocks_report(
        &blocks_report,
    ));
    tracing::info!(
        states = block_facts.len(),
        random_tick_states = block_facts.eligible_states(),
        "block simulation facts built from blocks report",
    );

    let entity_types = Arc::new(mc_data::entity_types::solaris_required_entity_types());
    tracing::info!(
        entries = entity_types.len(),
        "embedded entity type registry loaded"
    );
    let biome_spawns = Arc::new(mc_data::biomes::solaris_required_biome_spawn_rules());
    tracing::info!(
        biomes = biome_spawns.len(),
        "embedded biome spawn rules loaded"
    );

    let net = cfg
        .to_network(
            data,
            blocks,
            world,
            tags,
            recipes,
            loot,
            Some(block_light),
            items,
            item_facts,
            block_facts,
            entity_types,
            biome_spawns,
        )
        .with_context(|| format!("translating bind_address from {}", path.display()))?;
    tracing::info!(
        version = mc_server::VERSION,
        protocol = mc_protocol::PROTOCOL_VERSION,
        target = mc_protocol::TARGET_RELEASE,
        "Solaris starting",
    );

    // M29.f: race the network listener against a Ctrl-C signal. On
    // signal, request shutdown before the final save so gameplay tasks
    // stop mutating state first.
    let shutdown_handle = net.shutdown.clone();
    let bound = mc_net::bind(net).await.context("network bind")?;
    let save_handle = bound.save_handle();
    let mut run_fut = std::pin::pin!(bound.serve());
    let shutdown = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::warn!(error = %err, "ctrl_c handler failed; running without graceful shutdown");
            // Never resolve — let the listener own the lifetime.
            std::future::pending::<()>().await;
        }
    };
    tokio::select! {
        result = &mut run_fut => {
            result.context("network listener")
        }
        () = shutdown => {
            tracing::info!("shutdown signal received");
            let (drain_result, report) = request_shutdown_drain_then_save(
                &shutdown_handle,
                run_fut.as_mut(),
                save_handle.save_all(),
            ).await;
            match drain_result {
                Ok(Ok(())) => tracing::info!("shutdown: runtime tasks drained"),
                Ok(Err(err)) => return Err(err).context("network listener"),
                Err(_) => tracing::warn!("shutdown: drain timeout elapsed before final save"),
            }
            if report.is_ok() {
                tracing::info!(
                    players = report.players_saved,
                    entities = report.entities_saved,
                    chunks = report.chunks_flushed,
                    world_metadata = report.world_metadata_saved,
                    "shutdown: save-all complete"
                );
            } else {
                for error in &report.errors {
                    tracing::error!(%error, "shutdown: save-all error");
                }
                anyhow::bail!("shutdown save-all failed with {} error(s)", report.errors.len());
            }
            Ok(())
        }
    }
}

async fn request_shutdown_drain_then_save<D, S, SR>(
    shutdown: &mc_net::ShutdownHandle,
    drain: Pin<&mut D>,
    save: S,
) -> (Result<D::Output, tokio::time::error::Elapsed>, SR)
where
    D: Future,
    S: Future<Output = SR>,
{
    shutdown.request();
    let drain_result = tokio::time::timeout(SHUTDOWN_DRAIN_TIMEOUT, drain).await;
    let save_result = save.await;
    (drain_result, save_result)
}

fn build_terrain_generator(
    seed: i64,
    worldgen_mode: mc_worldgen::WorldgenMode,
    blocks: Arc<mc_world::BlockRegistry>,
    structure_rules: mc_worldgen::StructureRules,
) -> Arc<mc_worldgen::TerrainGenerator> {
    let biomes = mc_worldgen::BiomeRules::vanilla_overworld();

    let stone = blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").expect("static identifier"))
        .map(|block| block.default)
        .expect("block registry contains stone");
    let ores = mc_worldgen::OreRules::solaris_default(blocks.as_ref(), &biomes, stone);

    Arc::new(
        mc_worldgen::TerrainGenerator::with_rules(seed, blocks, biomes, ores)
            .with_mode(worldgen_mode)
            .with_structures(structure_rules),
    )
}

fn chunk_cache_size_for_view_distance(view_distance: i32) -> usize {
    let width = view_distance.max(0) as usize * 2 + 3;
    width * width
}

fn generate_spawn_window(
    storage: &mut mc_world::WorldStorage,
    generator: Arc<dyn mc_world::ChunkGenerator>,
    view_distance: i32,
    worker_threads: usize,
) -> Result<usize> {
    let view_distance = view_distance.max(0);
    let positions = spawn_window_positions(view_distance);
    let total = positions.len();
    if total == 0 {
        return Ok(0);
    }

    let workers = worker_threads.max(1).min(total);
    tracing::info!(
        chunks = total,
        workers,
        view_distance,
        "empty world pre-generation started",
    );

    let positions = Arc::new(positions);
    let next = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = std::sync::mpsc::channel();
    let batch_size = 8usize.min(total);
    for _ in 0..workers {
        let positions = Arc::clone(&positions);
        let next = Arc::clone(&next);
        let tx = tx.clone();
        let generator = Arc::clone(&generator);
        std::thread::spawn(move || {
            let mut batch = Vec::with_capacity(batch_size);
            loop {
                let idx = next.fetch_add(1, Ordering::Relaxed);
                let Some(&pos) = positions.get(idx) else {
                    break;
                };
                let chunk = generator.generate(pos);
                batch.push((pos, chunk));
                if batch.len() >= batch_size && tx.send(std::mem::take(&mut batch)).is_err() {
                    break;
                }
            }
            if !batch.is_empty() {
                let _ = tx.send(batch);
            }
        });
    }
    drop(tx);

    let started = Instant::now();
    let mut last_log = Instant::now();
    let log_every = (total / 20).max(64);
    let mut generated = 0usize;
    for batch in rx {
        for (pos, chunk) in batch {
            storage
                .insert_generated_chunk(pos, chunk)
                .with_context(|| format!("pre-generating spawn chunk ({}, {})", pos.x, pos.z))?;
            generated += 1;
        }
        if generated == total
            || generated.is_multiple_of(log_every)
            || last_log.elapsed() >= Duration::from_secs(2)
        {
            let percent = (generated * 90 / total).min(90);
            tracing::info!("Preparing world... {percent}%");
            tracing::info!(
                generated,
                total,
                elapsed_ms = started.elapsed().as_millis(),
                "empty world pre-generation progress",
            );
            last_log = Instant::now();
        }
    }

    let elapsed = started.elapsed();
    let chunks_per_second = generated as f64 / elapsed.as_secs_f64().max(0.001);
    tracing::info!(
        generated,
        total,
        elapsed_ms = elapsed.as_millis(),
        chunks_per_second,
        "empty world pre-generation finished",
    );
    Ok(generated)
}

fn spawn_window_positions(view_distance: i32) -> Vec<mc_world::ChunkPos> {
    let view_distance = view_distance.max(0);
    let width = view_distance as usize * 2 + 1;
    let mut positions = Vec::with_capacity(width * width);
    for z in -view_distance..=view_distance {
        for x in -view_distance..=view_distance {
            positions.push(mc_world::ChunkPos { x, z });
        }
    }
    positions
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

    #[tokio::test]
    async fn shutdown_sequence_requests_and_drains_before_save() {
        let shutdown = mc_net::ShutdownHandle::default();
        let events = Arc::new(std::sync::Mutex::new(Vec::new()));
        let drain_shutdown = shutdown.clone();
        let drain_events = Arc::clone(&events);
        let mut drain = std::pin::pin!(async move {
            assert!(drain_shutdown.is_requested());
            drain_events.lock().unwrap().push("drain");
            Ok::<(), std::io::Error>(())
        });
        let save_shutdown = shutdown.clone();
        let save_events = Arc::clone(&events);
        let save = async move {
            assert!(save_shutdown.is_requested());
            assert_eq!(save_events.lock().unwrap().as_slice(), ["drain"]);
            save_events.lock().unwrap().push("save");
            42
        };

        let (drain_result, save_result) =
            request_shutdown_drain_then_save(&shutdown, drain.as_mut(), save).await;

        assert!(drain_result.unwrap().is_ok());
        assert_eq!(save_result, 42);
        assert_eq!(events.lock().unwrap().as_slice(), ["drain", "save"]);
    }

    #[test]
    fn generate_spawn_window_materializes_view_square() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct StubGen {
            active: Arc<AtomicUsize>,
            max_active: Arc<AtomicUsize>,
        }

        impl mc_world::ChunkGenerator for StubGen {
            fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_active.fetch_max(active, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(5));
                let air = mc_world::BlockStateId(0);
                let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
                let mut chunk = mc_world::Chunk::empty(pos, air, biome);
                chunk.status = "minecraft:full".into();
                chunk.dirty = true;
                self.active.fetch_sub(1, Ordering::SeqCst);
                chunk
            }
        }

        let report = [mc_data::blocks::BlockReport {
            id: mc_data::Identifier::parse("minecraft:air").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        }];
        let registry = Arc::new(mc_world::BlockRegistry::from_report(&report).unwrap());
        let mut storage = mc_world::WorldStorage::in_memory_with_capacity(registry, 16);
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let generator = Arc::new(StubGen {
            active,
            max_active: Arc::clone(&max_active),
        });

        assert_eq!(
            generate_spawn_window(&mut storage, generator, 1, 4).unwrap(),
            9
        );
        assert_eq!(storage.cache_len(), 9);
        assert_eq!(storage.dirty_count(), 9);
        assert!(
            max_active.load(Ordering::SeqCst) > 1,
            "startup pre-generation should use worker threads"
        );
    }
}
