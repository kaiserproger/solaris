//! `mc-server` binary entry point.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

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
}

impl From<mc_net::ChunkPipelinePolicy> for EffectiveChunkPipeline {
    fn from(policy: mc_net::ChunkPipelinePolicy) -> Self {
        Self {
            chunk_io_threads: policy.chunk_io_threads,
            chunk_worker_threads: policy.chunk_worker_threads,
        }
    }
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
    let structure_rules = load_structure_rules(&cfg.data.vanilla_dir, blocks.as_ref())?;
    let chunk_pipeline = cfg.chunk_pipeline.to_network();
    let terrain_generator = build_terrain_generator(
        cfg.data.seed,
        &cfg.data.vanilla_dir,
        Arc::clone(&blocks),
        structure_rules,
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
                let mut storage = storage.with_generator(generator);
                let mut region_count = count_region_files(world_dir);
                if region_count == 0 {
                    let generated = generate_spawn_window(
                        &mut storage,
                        Arc::clone(&terrain_generator) as Arc<dyn mc_world::ChunkGenerator>,
                        cfg.server.view_distance,
                        chunk_pipeline.chunk_worker_threads,
                    )?;
                    let flushed = storage.flush_dirty()?;
                    region_count = count_region_files(world_dir);
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

    let entity_types = match mc_data::entity_types::load_entity_types_report(&items_path) {
        Ok(report) => {
            let reg = mc_data::entity_types::EntityTypeRegistry::from_report(&report);
            tracing::info!(
                entries = reg.len(),
                path = %items_path.display(),
                "entity type registry loaded",
            );
            Arc::new(reg)
        }
        Err(err) => {
            tracing::warn!(
                path = %items_path.display(),
                error = %err,
                "entity type registry load failed; passive mob spawning disabled",
            );
            Arc::new(mc_data::entity_types::EntityTypeRegistry::default())
        }
    };

    let biome_spawns_path = cfg.data.vanilla_dir.join("data/minecraft/worldgen/biome");
    let biome_spawns = match mc_data::biomes::load_biome_spawn_rules(&biome_spawns_path) {
        Ok(rules) => {
            tracing::info!(
                biomes = rules.len(),
                path = %biome_spawns_path.display(),
                "biome spawn rules loaded",
            );
            Arc::new(rules)
        }
        Err(err) => {
            tracing::warn!(
                path = %biome_spawns_path.display(),
                error = %err,
                "biome spawn rules load failed; passive mob spawning disabled",
            );
            Arc::new(mc_data::biomes::BiomeSpawnRules::default())
        }
    };

    let world_for_shutdown = net_world_clone(&world);
    let net = cfg
        .to_network(
            data,
            blocks,
            world,
            tags,
            block_light,
            items,
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

fn load_structure_rules(
    vanilla_dir: &Path,
    blocks: &mc_world::BlockRegistry,
) -> Result<mc_worldgen::StructureRules> {
    let root = vanilla_dir.join("data/minecraft/structure/village/plains");
    let mut template_paths = Vec::new();
    collect_nbt_templates(&root, &mut template_paths)?;
    template_paths.sort();
    if template_paths.is_empty() {
        tracing::warn!(
            path = %root.display(),
            "plains village templates missing; generated structures disabled",
        );
        return Ok(mc_worldgen::StructureRules::none());
    }

    let mut templates = Vec::new();
    for path in template_paths.into_iter().take(8) {
        match mc_worldgen::StructureTemplate::from_nbt_file(&path, blocks) {
            Ok(template) if !template.blocks().is_empty() => templates.push(template),
            Ok(_) => tracing::warn!(path = %path.display(), "empty structure template skipped"),
            Err(err) => tracing::warn!(
                path = %path.display(),
                error = %err,
                "structure template skipped",
            ),
        }
    }
    if templates.is_empty() {
        tracing::warn!(path = %root.display(), "no usable plains village templates loaded");
        return Ok(mc_worldgen::StructureRules::none());
    }
    let template_count = templates.len();
    let block_count: usize = templates
        .iter()
        .map(|template| template.blocks().len())
        .sum();
    tracing::info!(
        path = %root.display(),
        templates = template_count,
        blocks = block_count,
        "plains village templates loaded",
    );
    Ok(mc_worldgen::StructureRules::plains_village_markers(
        templates,
    ))
}

fn collect_nbt_templates(root: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(root)
        .with_context(|| format!("reading structure template directory {}", root.display()))?
    {
        let entry = entry.with_context(|| format!("reading entry under {}", root.display()))?;
        let path = entry.path();
        let ty = entry
            .file_type()
            .with_context(|| format!("reading file type for {}", path.display()))?;
        if ty.is_dir() {
            collect_nbt_templates(&path, out)?;
        } else if ty.is_file() && path.extension().is_some_and(|ext| ext == "nbt") {
            out.push(path);
        }
    }
    Ok(())
}

fn build_terrain_generator(
    seed: i64,
    vanilla_dir: &Path,
    blocks: Arc<mc_world::BlockRegistry>,
    structure_rules: mc_worldgen::StructureRules,
) -> Arc<mc_worldgen::TerrainGenerator> {
    let biome_dir = vanilla_dir.join("data/minecraft/worldgen/biome");
    let biome_tags_dir = vanilla_dir.join("data/minecraft/tags/worldgen/biome");
    let biome_data = match mc_data::biomes::load_biome_worldgen_data(&biome_dir, &biome_tags_dir) {
        Ok(data) => {
            tracing::info!(
                biomes = data.biomes().count(),
                tags = data.tags_len(),
                path = %biome_dir.display(),
                "biome worldgen data loaded",
            );
            Some(data)
        }
        Err(err) => {
            tracing::warn!(
                path = %biome_dir.display(),
                error = %err,
                "biome worldgen data load failed; using Solaris fallback biome rules",
            );
            None
        }
    };
    let biomes = biome_data
        .as_ref()
        .and_then(mc_worldgen::BiomeRules::from_worldgen_data)
        .unwrap_or_else(mc_worldgen::BiomeRules::vanilla_overworld);

    let stone = blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").expect("static identifier"))
        .map(|block| block.default)
        .expect("block registry contains stone");
    let ore_features =
        mc_data::worldgen_ores::load_ore_features(vanilla_dir.join("data/minecraft/worldgen"));
    let ores = match ore_features {
        Ok(features) => {
            match mc_worldgen::OreRules::from_features(
                blocks.as_ref(),
                &biomes,
                &features,
                biome_data.as_ref(),
            ) {
                Some(rules) => {
                    tracing::info!(
                        features = features.len(),
                        rules = rules.rules().len(),
                        "ore sidecar data fed into terrain generator",
                    );
                    rules
                }
                None => {
                    tracing::warn!(
                        features = features.len(),
                        "ore sidecar data produced no Solaris ore rules; using fallback",
                    );
                    mc_worldgen::OreRules::solaris_default(blocks.as_ref(), &biomes, stone)
                }
            }
        }
        Err(err) => {
            tracing::warn!(
                error = %err,
                "ore sidecar data load failed; using Solaris fallback ore rules",
            );
            mc_worldgen::OreRules::solaris_default(blocks.as_ref(), &biomes, stone)
        }
    };

    Arc::new(
        mc_worldgen::TerrainGenerator::with_rules(seed, blocks, biomes, ores)
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
    for _ in 0..workers {
        let positions = Arc::clone(&positions);
        let next = Arc::clone(&next);
        let tx = tx.clone();
        let generator = Arc::clone(&generator);
        std::thread::spawn(move || {
            loop {
                let idx = next.fetch_add(1, Ordering::Relaxed);
                let Some(&pos) = positions.get(idx) else {
                    break;
                };
                let chunk = generator.generate(pos);
                if tx.send((pos, chunk)).is_err() {
                    break;
                }
            }
        });
    }
    drop(tx);

    let started = Instant::now();
    let mut last_log = Instant::now();
    let log_every = (total / 20).max(64);
    let mut generated = 0usize;
    for (pos, chunk) in rx {
        storage
            .insert_generated_chunk(pos, chunk)
            .with_context(|| format!("pre-generating spawn chunk ({}, {})", pos.x, pos.z))?;
        generated += 1;
        if generated == total
            || generated.is_multiple_of(log_every)
            || last_log.elapsed() >= Duration::from_secs(2)
        {
            tracing::info!(
                generated,
                total,
                elapsed_ms = started.elapsed().as_millis(),
                "empty world pre-generation progress",
            );
            last_log = Instant::now();
        }
    }

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
