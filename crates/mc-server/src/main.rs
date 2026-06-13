//! `mc-server` binary entry point.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use std::{future::Future, pin::Pin};

use anyhow::{Context, Result, bail};
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
    effective_autoscale: EffectiveAutoscale,
}

impl<'a> From<&'a ServerConfig> for EffectiveConfig<'a> {
    fn from(config: &'a ServerConfig) -> Self {
        Self {
            config,
            effective_chunk_pipeline: EffectiveChunkPipeline::from(
                config.chunk_pipeline.to_network(),
            ),
            effective_autoscale: EffectiveAutoscale::from(config),
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

#[derive(serde::Serialize)]
struct EffectiveAutoscale {
    enabled: bool,
    runtime_mode: &'static str,
    profile: mc_server::AutoscaleProfile,
    initial_limits: EffectiveAutoscaleLimits,
    policy: EffectiveAutoscalePolicy,
}

impl From<&ServerConfig> for EffectiveAutoscale {
    fn from(config: &ServerConfig) -> Self {
        Self {
            enabled: config.autoscale.enabled,
            runtime_mode: "draft_noop_not_wired",
            profile: config.autoscale.profile,
            initial_limits: EffectiveAutoscaleLimits::from(
                config
                    .autoscale
                    .initial_limits(&config.server, &config.chunk_pipeline),
            ),
            policy: EffectiveAutoscalePolicy::from(
                config.autoscale.to_policy(&config.chunk_pipeline),
            ),
        }
    }
}

#[derive(serde::Serialize)]
struct EffectiveAutoscaleLimits {
    view_distance: i32,
    chunk_send_rate: u32,
    chunk_load_rate: u32,
    chunk_generate_rate: u32,
}

impl From<mc_net::RuntimeControlLimits> for EffectiveAutoscaleLimits {
    fn from(limits: mc_net::RuntimeControlLimits) -> Self {
        Self {
            view_distance: limits.view_distance,
            chunk_send_rate: limits.chunk_send_rate,
            chunk_load_rate: limits.chunk_load_rate,
            chunk_generate_rate: limits.chunk_generate_rate,
        }
    }
}

#[derive(serde::Serialize)]
struct EffectiveAutoscalePolicy {
    min_view_distance: i32,
    max_view_distance: i32,
    min_chunk_send_rate: u32,
    max_chunk_send_rate: u32,
    min_chunk_load_rate: u32,
    max_chunk_load_rate: u32,
    min_chunk_generate_rate: u32,
    max_chunk_generate_rate: u32,
    target_tick_ms: u64,
    target_first_chunk_ms: u64,
    queue_pressure_percent: u8,
    worker_pressure_percent: u8,
    memory_pressure_percent: u8,
    scale_down_after_ticks: u32,
    scale_up_after_ticks: u32,
}

impl From<mc_net::AutoscalePolicy> for EffectiveAutoscalePolicy {
    fn from(policy: mc_net::AutoscalePolicy) -> Self {
        Self {
            min_view_distance: policy.min_view_distance,
            max_view_distance: policy.max_view_distance,
            min_chunk_send_rate: policy.min_chunk_send_rate,
            max_chunk_send_rate: policy.max_chunk_send_rate,
            min_chunk_load_rate: policy.min_chunk_load_rate,
            max_chunk_load_rate: policy.max_chunk_load_rate,
            min_chunk_generate_rate: policy.min_chunk_generate_rate,
            max_chunk_generate_rate: policy.max_chunk_generate_rate,
            target_tick_ms: policy.target_tick_ms,
            target_first_chunk_ms: policy.target_first_chunk_ms,
            queue_pressure_percent: policy.queue_pressure_percent,
            worker_pressure_percent: policy.worker_pressure_percent,
            memory_pressure_percent: policy.memory_pressure_percent,
            scale_down_after_ticks: policy.scale_down_after_ticks,
            scale_up_after_ticks: policy.scale_up_after_ticks,
        }
    }
}

async fn serve(path: &Path) -> Result<()> {
    let cfg = load_config(path)?;

    let protocol_data = load_effective_protocol_data(cfg.data.vanilla_data_dir.as_deref())?;
    let data = protocol_data.data;
    tracing::info!(
        registries = data.registry_count(),
        entries = data.entry_count(),
        source = protocol_data.source,
        "registry index loaded",
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
    let tag_source = load_effective_tags(cfg.data.vanilla_data_dir.as_deref(), &data, &items)?;
    let tags = Arc::new(tag_source.tags);
    tracing::info!(
        tags = tags.total_tags(),
        entries = tags.total_entries(),
        source = tag_source.source,
        "tags loaded"
    );
    let recipes = Arc::new(mc_data::recipes::solaris_required_recipes());
    tracing::info!(entries = recipes.len(), "embedded recipe registry loaded");
    let loot_source = load_effective_loot(cfg.data.vanilla_data_dir.as_deref())?;
    let loot = Arc::new(loot_source.tables);
    tracing::info!(
        drops = loot.total_drops(),
        source = loot_source.source,
        "survival loot tables loaded"
    );

    let block_light_source =
        load_effective_block_light(cfg.data.vanilla_data_dir.as_deref(), &blocks_report)?;
    let block_light = Arc::new(block_light_source.table);
    tracing::info!(
        version = %block_light.version,
        states = block_light.len(),
        source = block_light_source.source,
        "block-light table loaded",
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

struct EffectiveLootTables {
    tables: mc_data::loot::LootTables,
    source: &'static str,
}

struct EffectiveProtocolData {
    data: mc_data::VanillaData,
    source: &'static str,
}

fn load_effective_protocol_data(vanilla_data_dir: Option<&Path>) -> Result<EffectiveProtocolData> {
    if let Some(vanilla_data_dir) = vanilla_data_dir {
        let data = mc_data::load(vanilla_data_dir).with_context(|| {
            format!(
                "loading vanilla registry data from {}",
                vanilla_data_dir.display()
            )
        })?;
        return Ok(EffectiveProtocolData {
            data,
            source: "vanilla_sidecar",
        });
    }

    Ok(EffectiveProtocolData {
        data: mc_data::solaris_required_data(),
        source: "embedded_solaris_fallback",
    })
}

struct EffectiveTags {
    tags: mc_data::tags::TagsData,
    source: &'static str,
}

fn load_effective_tags(
    vanilla_data_dir: Option<&Path>,
    data: &mc_data::VanillaData,
    items: &mc_data::items::ItemRegistry,
) -> Result<EffectiveTags> {
    if let Some(vanilla_data_dir) = vanilla_data_dir {
        let tags = mc_data::tags::load(vanilla_data_dir, data)
            .with_context(|| format!("loading vanilla tags from {}", vanilla_data_dir.display()))?;
        if tags.total_tags() == 0 {
            bail!(
                "vanilla tags from {} were empty; run tools/extract-vanilla-data.sh with tag data",
                vanilla_data_dir.display()
            );
        }
        for registry in ["minecraft:block", "minecraft:item", "minecraft:entity_type"] {
            let registry_id = mc_data::Identifier::parse(registry).expect("static registry id");
            let missing = match tags.registries.get(&registry_id) {
                Some(entries) => !entries.values().any(|ids| !ids.is_empty()),
                None => true,
            };
            if missing {
                bail!(
                    "vanilla tags from {} missing required resolved entries for tag registry {registry}",
                    vanilla_data_dir.display()
                );
            }
        }
        return Ok(EffectiveTags {
            tags,
            source: "vanilla_sidecar",
        });
    }

    Ok(EffectiveTags {
        tags: mc_data::tags::solaris_required_item_tags(items),
        source: "embedded_solaris_fallback",
    })
}

fn load_effective_loot(vanilla_data_dir: Option<&Path>) -> Result<EffectiveLootTables> {
    if let Some(vanilla_data_dir) = vanilla_data_dir {
        let root = vanilla_data_dir
            .join("data")
            .join("minecraft")
            .join("loot_table");
        let tables = mc_data::loot::load_vanilla_subset(&root)
            .with_context(|| format!("loading vanilla loot tables from {}", root.display()))?;
        if tables.total_drops() > 0 {
            return Ok(EffectiveLootTables {
                tables,
                source: "vanilla_sidecar_simple_subset",
            });
        }
        bail!(
            "vanilla loot tables from {} had no supported simple drops; run tools/extract-vanilla-data.sh with loot_table data",
            root.display()
        );
    }

    Ok(EffectiveLootTables {
        tables: mc_data::loot::builtin().clone(),
        source: "embedded_solaris_fallback",
    })
}

struct EffectiveBlockLight {
    table: mc_data::block_light::BlockLightTable,
    source: &'static str,
}

fn load_effective_block_light(
    vanilla_data_dir: Option<&Path>,
    blocks_report: &[mc_data::blocks::BlockReport],
) -> Result<EffectiveBlockLight> {
    if let Some(vanilla_data_dir) = vanilla_data_dir {
        let path = vanilla_data_dir.join("reports").join("block_light.json");
        let table = mc_data::block_light::load(&path).with_context(|| {
            format!("loading vanilla block-light table from {}", path.display())
        })?;
        if let Some(max_state_id) = blocks_report
            .iter()
            .flat_map(|block| block.states.iter().map(|state| state.id as usize))
            .max()
            && table.len() <= max_state_id
        {
            bail!(
                "vanilla block-light table from {} has {} states but blocks report requires state id {max_state_id}",
                path.display(),
                table.len()
            );
        }
        if table.version != mc_protocol::TARGET_RELEASE {
            bail!(
                "vanilla block-light table from {} targets {} but Solaris targets {}",
                path.display(),
                table.version,
                mc_protocol::TARGET_RELEASE
            );
        }
        return Ok(EffectiveBlockLight {
            table,
            source: "vanilla_sidecar",
        });
    }

    Ok(EffectiveBlockLight {
        table: mc_data::block_light::BlockLightTable::conservative_from_blocks_report(
            blocks_report,
        ),
        source: "embedded_solaris_fallback",
    })
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
    use mc_data::Identifier;
    use serde_json::Value;

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

    #[test]
    fn effective_protocol_data_rejects_missing_sidecar_root() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing-vanilla");

        let err = match load_effective_protocol_data(Some(&missing)) {
            Ok(_) => panic!("missing sidecar root must fail"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("loading vanilla registry data"));
    }

    #[test]
    fn effective_tags_reject_empty_vanilla_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let reports = tmp.path().join("reports");
        std::fs::create_dir_all(&reports).unwrap();
        std::fs::write(reports.join("registries.json"), "{}").unwrap();
        let data = mc_data::VanillaData::from_registries("", vec![]);
        let items = mc_data::items::ItemRegistry::default();

        let err = match load_effective_tags(Some(tmp.path()), &data, &items) {
            Ok(_) => panic!("empty vanilla tag sidecar must fail"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("vanilla tags"));
        assert!(err.to_string().contains("were empty"));
    }

    #[test]
    fn effective_tags_reject_missing_required_vanilla_tag_registry() {
        let tmp = tempfile::tempdir().unwrap();
        let reports = tmp.path().join("reports");
        std::fs::create_dir_all(&reports).unwrap();
        std::fs::write(
            reports.join("registries.json"),
            r#"{
                "minecraft:item": {
                    "entries": {
                        "minecraft:apple": { "protocol_id": 5 }
                    }
                }
            }"#,
        )
        .unwrap();
        let tags_item = tmp
            .path()
            .join("data")
            .join("minecraft")
            .join("tags")
            .join("item");
        std::fs::create_dir_all(&tags_item).unwrap();
        std::fs::write(
            tags_item.join("food.json"),
            r#"{ "values": [ "minecraft:apple" ] }"#,
        )
        .unwrap();
        let data = mc_data::VanillaData::from_registries("", vec![]);
        let items = mc_data::items::ItemRegistry::default();

        let err = match load_effective_tags(Some(tmp.path()), &data, &items) {
            Ok(_) => panic!("partial vanilla tag sidecar must fail"),
            Err(err) => err,
        };

        assert!(
            err.to_string()
                .contains("missing required resolved entries")
        );
        assert!(err.to_string().contains("minecraft:block"));
    }

    #[test]
    fn effective_tags_reject_required_tag_registries_without_protocol_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let reports = tmp.path().join("reports");
        std::fs::create_dir_all(&reports).unwrap();
        std::fs::write(reports.join("registries.json"), "{}").unwrap();
        for (root, entry) in [
            ("block", "minecraft:stone"),
            ("item", "minecraft:apple"),
            ("entity_type", "minecraft:pig"),
        ] {
            let tags_root = tmp
                .path()
                .join("data")
                .join("minecraft")
                .join("tags")
                .join(root);
            std::fs::create_dir_all(&tags_root).unwrap();
            std::fs::write(
                tags_root.join("sample.json"),
                format!(r#"{{ "values": [ "{entry}" ] }}"#),
            )
            .unwrap();
        }
        let data = mc_data::VanillaData::from_registries("", vec![]);
        let items = mc_data::items::ItemRegistry::default();

        let err = match load_effective_tags(Some(tmp.path()), &data, &items) {
            Ok(_) => panic!("unresolved required tag registries must fail"),
            Err(err) => err,
        };

        assert!(
            err.to_string()
                .contains("missing required resolved entries")
        );
    }

    #[test]
    fn effective_block_light_requires_sidecar_file_when_vanilla_dir_is_set() {
        let tmp = tempfile::tempdir().unwrap();
        let report = [mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        }];

        let err = match load_effective_block_light(Some(tmp.path()), &report) {
            Ok(_) => panic!("missing block_light.json must fail"),
            Err(err) => err,
        };

        assert!(
            err.to_string()
                .contains("loading vanilla block-light table")
        );
        assert!(err.to_string().contains("block_light.json"));
    }

    #[test]
    fn effective_block_light_rejects_sidecar_that_does_not_cover_blocks_report() {
        let tmp = tempfile::tempdir().unwrap();
        let reports = tmp.path().join("reports");
        std::fs::create_dir_all(&reports).unwrap();
        std::fs::write(
            reports.join("block_light.json"),
            r#"{"version":"26.1.2-test","max_state_id":0,"entries":[[0,0,1]]}"#,
        )
        .unwrap();
        let report = [mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:stone").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 1,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        }];

        let err = match load_effective_block_light(Some(tmp.path()), &report) {
            Ok(_) => panic!("stale block-light sidecar must fail"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("requires state id 1"));
    }

    #[test]
    fn effective_block_light_rejects_wrong_target_version() {
        let tmp = tempfile::tempdir().unwrap();
        let reports = tmp.path().join("reports");
        std::fs::create_dir_all(&reports).unwrap();
        std::fs::write(
            reports.join("block_light.json"),
            r#"{"version":"not-the-target","max_state_id":0,"entries":[[0,0,1]]}"#,
        )
        .unwrap();
        let report = [mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        }];

        let err = match load_effective_block_light(Some(tmp.path()), &report) {
            Ok(_) => panic!("wrong block-light target version must fail"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("not-the-target"));
        assert!(err.to_string().contains(mc_protocol::TARGET_RELEASE));
    }

    #[test]
    fn effective_block_light_uses_embedded_fallback_without_sidecar() {
        let report = [mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: std::collections::BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: std::collections::BTreeMap::new(),
            }],
        }];

        let light = load_effective_block_light(None, &report).unwrap();

        assert_eq!(light.source, "embedded_solaris_fallback");
        assert_eq!(light.table.version, "blocks-report-conservative");
        assert_eq!(light.table.len(), 1);
    }

    #[test]
    fn effective_loot_uses_embedded_fallback_without_sidecar() {
        let loot = load_effective_loot(None).unwrap();

        assert_eq!(loot.source, "embedded_solaris_fallback");
        assert_eq!(
            loot.tables
                .block_drop(&Identifier::parse("minecraft:stone").unwrap()),
            Some(&Identifier::parse("minecraft:cobblestone").unwrap())
        );
    }

    #[test]
    fn effective_loot_uses_simple_vanilla_sidecar_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        let blocks = tmp
            .path()
            .join("data")
            .join("minecraft")
            .join("loot_table")
            .join("blocks");
        std::fs::create_dir_all(&blocks).unwrap();
        std::fs::write(
            blocks.join("stone.json"),
            r#"{
              "pools": [{
                "entries": [{
                  "type": "minecraft:item",
                  "name": "minecraft:cobblestone"
                }]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_effective_loot(Some(tmp.path())).unwrap();

        assert_eq!(loot.source, "vanilla_sidecar_simple_subset");
        assert_eq!(loot.tables.total_drops(), 1);
        assert_eq!(
            loot.tables
                .block_drop(&Identifier::parse("minecraft:stone").unwrap()),
            Some(&Identifier::parse("minecraft:cobblestone").unwrap())
        );
    }

    #[test]
    fn effective_loot_rejects_sidecar_with_no_simple_loot() {
        let tmp = tempfile::tempdir().unwrap();

        let err = match load_effective_loot(Some(tmp.path())) {
            Ok(_) => panic!("configured vanilla loot sidecar without usable drops must fail"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("vanilla loot tables"));
        assert!(err.to_string().contains("no supported simple drops"));
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
                chunk.mark_dirty();
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

    #[test]
    fn check_output_marks_autoscale_draft_noop_and_normalized_bounds() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"
            view_distance = 12

            [network]
            bind_address = "0.0.0.0"
            port = 25565

            [chunk_pipeline]
            chunk_send_rate = 3
            chunk_load_rate = 5
            chunk_generate_rate = 7

            [autoscale]
            enabled = true
            min_view_distance = 0
            max_view_distance = 1
            scale_down_after_ticks = 0
            scale_up_after_ticks = 0
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
        let rendered = serde_json::to_value(EffectiveConfig::from(&cfg)).expect("serialize");
        let autoscale = &rendered["effective_autoscale"];
        let policy = &autoscale["policy"];

        assert_eq!(autoscale["enabled"], Value::Bool(true));
        assert_eq!(autoscale["runtime_mode"], "draft_noop_not_wired");
        assert_eq!(policy["min_view_distance"], 2);
        assert_eq!(policy["max_view_distance"], 2);
        assert_eq!(policy["min_chunk_send_rate"], 3);
        assert_eq!(policy["max_chunk_send_rate"], 16);
        assert_eq!(policy["min_chunk_load_rate"], 5);
        assert_eq!(policy["max_chunk_load_rate"], 64);
        assert_eq!(policy["min_chunk_generate_rate"], 7);
        assert_eq!(policy["max_chunk_generate_rate"], 32);
        assert_eq!(policy["scale_down_after_ticks"], 1);
        assert_eq!(policy["scale_up_after_ticks"], 1);
    }
}
