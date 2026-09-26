//! `mc-server` binary entry point.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use mc_server::{OperatorFileOperation, PluginHookSection, ServerConfig};

mod startup_validation;

use mc_server::startup_data::{
    StartupData, load_effective_loot, load_effective_protocol_data, load_effective_recipes,
    load_effective_tags,
};
use startup::*;

#[cfg(test)]
use startup_validation::ensure_world_contract;
#[cfg(test)]
use startup_validation::{PersistedWorldContract, WORLD_CONTRACT_SCHEMA, world_contract_path};
use startup_validation::{
    WorldSource, ensure_world_contract_with_spawn, has_non_directory_ancestor, is_public_bind_ip,
    required_world_dir, validate_runtime_config, world_region_root_is_blocked,
    world_requires_solaris_spawn,
};

const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(6);
/// Upper bound on one `pregenerate` request, so a typo cannot ask for a
/// planet-sized world.
const MAX_PREGENERATE_CHUNKS: usize = 4_194_304; // 2048 x 2048 chunks
const STARTUP_LIGHT_BAKE_WORKER_CAP: usize = 16;
const STARTUP_GENERATION_QUEUE_BATCHES: usize = 8;

mod console;
mod content_cache;
mod content_import;
mod startup;
mod startup_rules;

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

    /// Use plain stdin commands instead of the interactive terminal console.
    #[arg(long)]
    no_console: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Manage identities in the configured persisted operator file.
    Operator {
        #[command(subcommand)]
        command: OperatorCommand,
    },
    /// Generate and store every chunk of a block-coordinate rectangle, then exit.
    ///
    /// Smaller than the configured view distance still writes the startup spawn
    /// window, and already stored chunks are simply regenerated identically.
    Pregenerate {
        /// Inclusive first corner, as `x,z` block coordinates.
        #[arg(long, value_parser = parse_block_coords, allow_hyphen_values = true)]
        from: (i32, i32),
        /// Inclusive opposite corner, as `x,z` block coordinates.
        #[arg(long, value_parser = parse_block_coords, allow_hyphen_values = true)]
        to: (i32, i32),
    },
    /// Manage the vanilla content cache the server runs on.
    Content {
        #[command(subcommand)]
        command: ContentCommand,
    },
}

#[derive(Debug, Subcommand)]
enum ContentCommand {
    /// Derive a complete vanilla content cache from your own licensed
    /// Minecraft Java installation and publish it atomically.
    ///
    /// Nothing is redistributed: the artifact is taken from `--from`, or
    /// downloaded from Mojang's public metadata and verified against the
    /// version's size and SHA-1, and the derived cache stays in a local,
    /// gitignored directory. A JDK is required for the derivation steps.
    Import {
        /// Release to derive. Solaris targets a single release.
        #[arg(long, default_value = mc_protocol::TARGET_RELEASE)]
        version: String,
        /// Use a local server bundle jar instead of downloading one.
        #[arg(long, conflicts_with = "download")]
        from: Option<PathBuf>,
        /// Download the artifact from Mojang's public metadata (the default).
        #[arg(long)]
        download: bool,
        /// Cache root to publish. Defaults to the discovered location.
        #[arg(long)]
        cache: Option<PathBuf>,
    },
}

fn parse_block_coords(raw: &str) -> Result<(i32, i32), String> {
    let (x, z) = raw
        .split_once(',')
        .ok_or_else(|| format!("expected `x,z` block coordinates, got `{raw}`"))?;
    let x = x
        .trim()
        .parse::<i32>()
        .map_err(|error| format!("invalid block x in `{raw}`: {error}"))?;
    let z = z
        .trim()
        .parse::<i32>()
        .map_err(|error| format!("invalid block z in `{raw}`: {error}"))?;
    Ok((x, z))
}

#[derive(Debug, Subcommand)]
enum OperatorCommand {
    /// Add a Minecraft username or UUID.
    Add { identity: String },
    /// Remove a Minecraft username or UUID.
    Remove { identity: String },
    /// List all persisted operator identities in deterministic order.
    List,
}

fn load_config(path: &Path) -> Result<ServerConfig> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading config file {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parsing config file {}", path.display()))
}

async fn serve(
    path: &Path,
    warning_ring: Arc<mc_server::dashboard_stats::WarningRing>,
    console_output: Option<(console::ConsoleOutput, bool)>,
    pregenerate: Option<((i32, i32), (i32, i32))>,
) -> Result<()> {
    let mut cfg = load_config(path)?;
    // Resolve and bound the requested rectangle before the world is touched, so
    // an oversized request cannot create or generate anything.
    let pregenerate = match pregenerate {
        Some((from, to)) => Some((from, to, region_positions(from, to)?)),
        None => None,
    };
    let access_control = cfg.load_access_control_files(path)?;
    if access_control.files_loaded > 0 {
        tracing::info!(
            files = access_control.files_loaded,
            operators = access_control.operator_identities,
            whitelist = access_control.whitelist_identities,
            banned = access_control.banned_identities,
            "file-backed access control loaded"
        );
    }
    validate_runtime_config(&cfg)?;
    let configured_geometry = cfg.data.chunk_geometry().map_err(anyhow::Error::msg)?;
    let world_dir = required_world_dir(&cfg)?;
    let worldgen_mode = cfg.data.worldgen_mode.to_worldgen();
    let mut component_deployment_prepared = prepare_component_deployment(&cfg).await?;
    let mut dashboard_plugin_ids = Vec::new();
    if let Some(component) = component_deployment_prepared.as_ref() {
        for id in &component.plugin_ids {
            dashboard_plugin_ids.push(id.clone());
            let client_bundles = component
                .client_bundles
                .iter()
                .filter(|bundle| bundle.owner_plugin_id() == id.as_str())
                .map(mc_script::ClientBundle::id)
                .collect::<Vec<_>>();
            tracing::info!(
                plugin_id = id,
                runtime = "wasm",
                client_bundles = ?client_bundles,
                "component plugin discovered"
            );
        }
        // The records are read from the running host, so what the operator sees
        // here is what the world contract below will record.
        tracing::info!(
            declares_startup_rules = component.rules.is_some(),
            ore_profile = component
                .ore_profile
                .map(mc_script::PluginWorldgenOreProfile::contract_name),
            settlement_profile = component
                .settlement_plan
                .as_ref()
                .map(mc_script::PluginSettlementPlan::contract_name),
            client_bundles = component.client_bundles.len(),
            "component plugin deployment records read back"
        );
    }
    let records = DeploymentRecords::of(component_deployment_prepared.as_ref());
    let identities = records.world_contract_identities(cfg.data.settlement_profile);

    // Component package bundles are the ones their manifests declared and whose
    // artifacts were verified against.
    let loader_manifest = records.loader_manifest()?;
    if !records.items.is_empty() {
        loader_manifest
            .as_ref()
            .context("custom items require a Loader bundle with register_items permission")?
            .validate_custom_items(records.items)
            .context("checking custom item Loader declarations")?;
    }
    let content = content_import::resolve_or_import(&content_search(&cfg)).await?;
    let vanilla_data_dir = content.root();
    let StartupData {
        data,
        blocks,
        block_light,
        items,
        item_facts,
        tags,
        recipes,
        loot,
        block_facts,
        entity_types,
        mut biome_spawns,
    } = StartupData::load(vanilla_data_dir, loader_manifest.as_deref())?;
    let item_facts = Arc::new(
        Arc::unwrap_or_clone(item_facts)
            .with_custom_items(records.items.iter().cloned(), &items)
            .context("validating configured custom item identities")?,
    );
    let structure_rules = structure_rules_for_startup(
        cfg.data.seed,
        cfg.data.worldgen_mode,
        vanilla_data_dir,
        &blocks,
        &items,
        records.settlement_plan,
        cfg.data.settlement_profile,
    )?;
    let mut recipes = Arc::unwrap_or_clone(recipes);
    for item in records.items {
        if let Some(ingredient) = &item.crafting_ingredient {
            recipes.push(mc_data::recipes::Recipe {
                id: item.id.clone(),
                kind: mc_data::recipes::RecipeKind::Shapeless(mc_data::recipes::ShapelessRecipe {
                    ingredients: vec![mc_data::recipes::Ingredient {
                        alternatives: vec![mc_data::recipes::IngredientAlternative::Item(
                            ingredient.clone(),
                        )],
                    }],
                }),
                result: mc_data::recipes::RecipeResult {
                    item: item.id.clone(),
                    count: 1,
                    stew_effects: Vec::new(),
                },
            });
        }
    }
    let recipes = Arc::new(recipes);
    let chunk_pipeline = cfg.chunk_pipeline.to_network();
    let chest_loot =
        chest_loot_catalog_for_startup(vanilla_data_dir).map(|catalog| (catalog, (*items).clone()));
    let village_plans = village_plan_source_for_startup(
        cfg.data.seed,
        vanilla_data_dir,
        &blocks,
        &data,
        &tags,
        records.settlement_plan,
        cfg.data.settlement_profile,
    )?;
    let mut terrain_generator = build_terrain_generator(
        cfg.data.seed,
        worldgen_mode,
        configured_geometry,
        Arc::clone(&blocks),
        structure_rules,
        records.ore_profile,
        chest_loot,
        village_plans,
    )?;
    // The rules the world is opened with, from the one runtime this deployment
    // runs. A component deployment's plan was converted and validated while its
    // host started, so a refusal has already failed the server by now.
    if let Some(rules) = records.rules {
        startup_rules::apply(
            rules,
            Arc::get_mut(&mut terrain_generator).expect("terrain is unpublished during startup"),
            Arc::make_mut(&mut biome_spawns),
            &entity_types,
        )
        .context("materializing startup rules")?;
    }
    let configured_spawn = if world_requires_solaris_spawn(world_dir)? {
        let located = terrain_generator
            .locate_safe_spawn()
            .context("finding a bounded natural spawn in generated terrain")?;
        mc_world::WorldSpawn::new(located.block_x, located.block_z)
    } else {
        mc_world::WorldSpawn::default()
    };
    let world_source = ensure_world_contract_with_spawn(
        world_dir,
        configured_geometry,
        cfg.data.seed,
        worldgen_mode.contract_name(),
        &identities.ore_profile,
        &identities.settlement_profile,
        configured_spawn,
        identities.gameplay_rules.as_deref(),
        identities.custom_items.as_deref(),
    )?;
    let world_spawn = match world_source {
        WorldSource::SolarisGenerated => configured_spawn,
        WorldSource::ExistingVanilla => mc_world::WorldSpawn::default(),
    };
    tracing::info!(
        block_x = world_spawn.block_x,
        block_z = world_spawn.block_z,
        chunk_x = world_spawn.chunk().x,
        chunk_z = world_spawn.chunk().z,
        source = ?world_source,
        "world spawn centre resolved",
    );

    let startup_view_distance = startup_spawn_view_distance(&cfg);
    let cache_view_distance = runtime_cache_view_distance(&cfg);
    tracing::info!(
        configured_view_distance = cfg.server.view_distance,
        startup_view_distance,
        cache_view_distance,
        autoscale_enabled = cfg.autoscale.enabled,
        "startup spawn preparation policy resolved",
    );

    let world: Option<mc_net::WorldHandle> = if let Some(world_dir) = &cfg.data.world_dir {
        let open_result = (|| -> Result<mc_world::WorldStorage> {
            ensure_world_region_root(world_dir)?;
            Ok(mc_world::WorldStorage::open_with_capacities(
                world_dir,
                Arc::clone(&blocks),
                chunk_cache_size_for_view_distance(cache_view_distance),
                chunk_pipeline.region_cache_size,
            )?)
        })();
        match open_result {
            Ok(storage) => {
                // Solaris worlds generate missing chunks. Imported vanilla
                // worlds stay read-only with respect to terrain authority.
                let startup_workers =
                    startup_chunk_worker_threads(chunk_pipeline.chunk_worker_threads);
                let startup_light_workers = startup_light_bake_worker_threads(startup_workers);
                let mut storage = storage
                    .with_item_registry(Arc::clone(&items))
                    .with_spawn(world_spawn);
                if world_source == WorldSource::SolarisGenerated {
                    let generator: Arc<dyn mc_world::ChunkGenerator> =
                        Arc::clone(&terrain_generator) as Arc<dyn mc_world::ChunkGenerator>;
                    storage = storage.with_generator(generator);
                }
                let mut region_count = count_region_files(world_dir);
                if region_count == 0 {
                    if world_source == WorldSource::ExistingVanilla {
                        bail!("existing vanilla world has no readable overworld region files");
                    }
                    let generated = generate_spawn_window(
                        &mut storage,
                        Arc::clone(&terrain_generator) as Arc<dyn mc_world::ChunkGenerator>,
                        startup_view_distance,
                        startup_workers,
                        startup_light_workers,
                        Some(block_light.as_ref()),
                    )?;
                    tracing::info!("Preparing world... 95% (spawn window resident)");
                    region_count = count_region_files(world_dir);
                    tracing::info!("Preparing world... 100%");
                    tracing::info!(
                        path = %world_dir.display(),
                        chunks = generated,
                        dirty = storage.dirty_count(),
                        region_files = region_count,
                        "empty world pre-generated around spawn; disk flush deferred to startup checkpoint",
                    );
                } else {
                    let prepared = prepare_existing_spawn_window(
                        &mut storage,
                        block_light.as_ref(),
                        startup_view_distance,
                        startup_light_workers,
                    )?;
                    tracing::info!(
                        path = %world_dir.display(),
                        chunks = prepared.warmed,
                        baked = prepared.baked,
                        flushed = 0usize,
                        dirty = prepared.dirty,
                        configured_view_distance = cfg.server.view_distance,
                        startup_view_distance,
                        "existing world startup spawn window warmed",
                    );
                }
                tracing::info!(
                    path = %world_dir.display(),
                    block_count = storage.registry().len(),
                    region_files = region_count,
                    seed = cfg.data.seed,
                    source = ?world_source,
                    "world storage opened with worldgen baseline",
                );
                if let Some((from, to, positions)) = pregenerate {
                    ensure_pregenerate_target(world_source)?;
                    let requested = positions.len();
                    let (pending, skipped) = pending_region_positions(&mut storage, positions)?;
                    let generated = generate_chunk_positions(
                        &mut storage,
                        Arc::clone(&terrain_generator) as Arc<dyn mc_world::ChunkGenerator>,
                        pending,
                        startup_workers,
                        "region",
                    )?;
                    let flushed = storage.flush_dirty()?;
                    tracing::info!(
                        from = ?from,
                        to = ?to,
                        chunks = requested,
                        generated,
                        skipped,
                        flushed,
                        "region pre-generation finished; every chunk is on disk",
                    );
                    return Ok(());
                }
                Some(Arc::new(tokio::sync::Mutex::new(storage)))
            }
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("opening configured world directory {}", world_dir.display())
                });
            }
        }
    } else {
        bail!("data.world_dir is required to start a playable persistent server");
    };

    let mut net = cfg
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
    net.loader_manifest = loader_manifest;
    if let Some(manifest) = net.loader_manifest.as_deref() {
        tracing::info!(
            protocol = manifest.protocol,
            bundles = manifest.bundles.len(),
            "Solaris Loader manifest prepared"
        );
    }
    tracing::info!(
        version = mc_server::VERSION,
        protocol = mc_protocol::PROTOCOL_VERSION,
        target = mc_protocol::TARGET_RELEASE,
        "Solaris starting",
    );

    let shutdown_handle = net.shutdown.clone();
    let mut component_host = None;
    let bound = if let Some(mut prepared) = component_deployment_prepared.take() {
        // The component host is already running: it was started before the world
        // was opened, because that is when its `configure` phase produced the
        // rules the world was opened with.
        let loaded = prepared.plugin_ids.len();
        let sessions = prepared.sessions.clone();
        let host = prepared.take_host();
        tracing::info!(
            plugin_directory = ?cfg.plugins.directory.as_deref(),
            loaded,
            "component plugin host started"
        );
        match mc_net::bind_with_scripts(net, host.boundary().clone()).await {
            Ok(bound) => {
                // The server now owns the sessions the host holds the lookup for,
                // which is what makes a plugin's uuid-addressed command reach the
                // connection that identity has right now.
                bound.register_player_sessions(&sessions);
                component_host = Some(host);
                bound
            }
            Err(error) => {
                stop_component_host(host).await?;
                return Err(error).context("network bind");
            }
        }
    } else {
        mc_net::bind(net).await.context("network bind")?
    };
    let stats = (cfg.dashboard.enabled || console_output.is_some()).then(|| {
        Arc::new(mc_server::dashboard_stats::ServerDashboardStats::new(
            &bound,
            &cfg,
            std::time::Instant::now(),
            warning_ring,
            dashboard_plugin_ids,
        ))
    });
    let dashboard_task = if let Some(stats) = stats.as_ref().filter(|_| cfg.dashboard.enabled) {
        let socket = cfg.dashboard.validate().map_err(anyhow::Error::msg)?;
        tracing::info!(endpoint = %socket, "operator dashboard enabled; binding in background");
        Some(mc_server::dashboard::spawn_dashboard(
            mc_server::dashboard::DashboardListenConfig {
                bind_address: socket.ip(),
                port: socket.port(),
            },
            Arc::clone(stats) as Arc<dyn mc_server::dashboard::DashboardStats>,
        ))
    } else {
        None
    };
    let terminal_console = console_output
        .zip(stats)
        .map(|((output, interactive), stats)| {
            let stats: Arc<dyn mc_server::dashboard::DashboardStats> = stats;
            console::Console {
                handler: console::server_commands::ServerCommands {
                    stats: Arc::clone(&stats),
                    save: bound.save_handle(),
                    control: bound.operator_control_handle(),
                    config_path: path.to_path_buf(),
                },
                stats,
                ticks: bound
                    .runtime_telemetry_handle()
                    .subscribe_simulation_ticks(),
                output,
                interactive,
            }
        });
    let plugin_host = component_host.as_ref();
    let result = run_bound_server(
        bound,
        shutdown_handle,
        path,
        plugin_host,
        cfg.plugins.strict,
        terminal_console,
    )
    .await;
    if let Some(task) = dashboard_task {
        task.abort();
    }
    if let Some(host) = component_host {
        stop_component_host(host).await?;
    }
    result
}

#[cfg(unix)]
struct PluginReloadSignal(tokio::signal::unix::Signal);

#[cfg(not(unix))]
struct PluginReloadSignal;

impl PluginReloadSignal {
    #[cfg(unix)]
    fn new() -> Result<Self> {
        let signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .context("installing SIGHUP plugin reload handler")?;
        Ok(Self(signal))
    }

    #[cfg(not(unix))]
    fn new() -> Result<Self> {
        Ok(Self)
    }

    #[cfg(unix)]
    async fn recv(&mut self) {
        let _ = self.0.recv().await;
    }

    #[cfg(not(unix))]
    async fn recv(&mut self) {
        std::future::pending::<()>().await;
    }
}

fn prepare_configured_component_reload(
    path: &Path,
    startup_strict: bool,
) -> Result<Vec<mc_plugin_host::LoadedPackage>> {
    if !startup_strict {
        bail!("component runtime reload requires the server to start with plugins.strict = true");
    }
    let config = load_config(path)?;
    if !config.plugins.strict {
        bail!("reloaded config must keep plugins.strict = true");
    }
    let deployment = component_deployment(&config)?
        .context("reloaded config no longer enables a component plugin host")?;
    let limits = mc_plugin_host::PluginLimits::default();
    let discovered = mc_plugin_host::discover(&deployment, &limits).with_context(|| {
        format!(
            "reading the configured component plugins from {}",
            deployment.root.display()
        )
    })?;
    for skipped in discovered.skipped() {
        tracing::warn!(
            path = %skipped.path.display(),
            message = skipped.message,
            "component plugin skipped"
        );
    }
    Ok(discovered.into_packages())
}

async fn reload_configured_component_plugins(
    path: PathBuf,
    startup_strict: bool,
    host: &mc_plugin_host::PluginHost,
) -> Result<mc_plugin_host::PluginReloadReport> {
    let packages = tokio::task::spawn_blocking(move || {
        prepare_configured_component_reload(&path, startup_strict)
    })
    .await
    .context("joining component reload preparation task")??;
    host.reload(packages)
        .await
        .context("committing prepared component plugin reload")
}

async fn run_bound_server(
    bound: mc_net::BoundServer,
    shutdown_handle: mc_net::ShutdownHandle,
    config_path: &Path,
    plugin_host: Option<&mc_plugin_host::PluginHost>,
    startup_plugin_strict: bool,
    terminal_console: Option<console::Console<console::server_commands::ServerCommands>>,
) -> Result<()> {
    // Every exit path drains admitted work and performs exactly one final save.
    // Ctrl-C and SIGTERM only request shutdown; they then wait for that same
    // lifecycle, so `kill` and container/supervisor stops save exactly like the
    // console does. SIGHUP prepares/reloads plugins without pausing the network
    // future while files are read.
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("installing SIGTERM handler")?;
    let mut run_fut = std::pin::pin!(bound.serve_and_save());
    let mut shutdown = Box::pin(async {
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.context("installing shutdown signal handler"),
            () = async {
                #[cfg(unix)]
                let _ = terminate.recv().await;
                #[cfg(not(unix))]
                std::future::pending::<()>().await;
            } => Ok(()),
            result = async {
                match terminal_console {
                    Some(console) => console.run().await,
                    None => std::future::pending().await,
                }
            } => result,
        }
    });
    let mut plugin_reload = PluginReloadSignal::new()?;
    loop {
        tokio::select! {
            result = run_fut.as_mut() => {
                return result.context("network listener");
            }
            console_result = shutdown.as_mut() => {
                tracing::info!("shutdown requested");
                shutdown_handle.request();
                let result = match tokio::time::timeout(SHUTDOWN_DRAIN_TIMEOUT, run_fut.as_mut()).await {
                    Ok(result) => result.context("network listener"),
                    Err(_) => Err(anyhow::anyhow!("shutdown drain and final save timed out")),
                };
                console_result?;
                return result;
            }
            () = plugin_reload.recv() => {
                let Some(host) = plugin_host else {
                    tracing::warn!("SIGHUP plugin reload ignored because no plugin host is configured");
                    continue;
                };
                if !startup_plugin_strict {
                    tracing::warn!(
                        "SIGHUP plugin reload rejected because the server did not start with plugins.strict = true"
                    );
                    continue;
                }
                tracing::info!(
                    config = %config_path.display(),
                    "SIGHUP component plugin reload requested"
                );
                let mut reload = Box::pin(reload_configured_component_plugins(
                    config_path.to_path_buf(),
                    startup_plugin_strict,
                    host,
                ));
                let reload_result = tokio::select! {
                    result = run_fut.as_mut() => {
                        return result.context("network listener");
                    }
                    console_result = shutdown.as_mut() => {
                        tracing::info!("shutdown requested during component reload");
                        shutdown_handle.request();
                        let result = match tokio::time::timeout(
                            SHUTDOWN_DRAIN_TIMEOUT,
                            run_fut.as_mut(),
                        )
                        .await
                        {
                            Ok(result) => result.context("network listener"),
                            Err(_) => Err(anyhow::anyhow!("shutdown drain and final save timed out")),
                        };
                        console_result?;
                        return result;
                    }
                    result = reload.as_mut() => result,
                };
                match reload_result {
                    Ok(report) => {
                        tracing::info!(
                            loaded = report.loaded_packages,
                            replaced = report.replaced.len(),
                            "component plugin reload committed"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "component plugin reload rejected");
                    }
                }
            }
        }
    }
}

fn init_tracing() -> Result<Arc<mc_server::dashboard_stats::WarningRing>> {
    use tracing_subscriber::filter::LevelFilter;
    use tracing_subscriber::prelude::*;
    std::fs::create_dir_all("logs").context("creating logs directory")?;
    let latest = std::fs::File::create("logs/latest.log").context("opening logs/latest.log")?;
    let debug = std::fs::File::create("logs/debug.log").context("opening logs/debug.log")?;

    let ring = mc_server::dashboard_stats::warning_ring();
    let ring_layer = tracing_subscriber::fmt::layer()
        .with_writer(mc_server::dashboard_stats::WarningRingSink(Arc::clone(
            &ring,
        )))
        .with_ansi(false);
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("debug"));
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .compact()
                .with_target(false)
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(latest))
                .with_filter(LevelFilter::INFO),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(debug))
                .with_filter(filter),
        )
        .with(
            (std::env::var("SOLARIS_HARNESS_LOG_STDOUT").as_deref() == Ok("1")).then(|| {
                tracing_subscriber::fmt::layer()
                    .compact()
                    .with_target(false)
                    .with_ansi(false)
                    .with_filter(LevelFilter::INFO)
            }),
        )
        .with(ring_layer.with_filter(LevelFilter::WARN))
        .init();
    Ok(ring)
}

/// Resolve the content cache for this machine/config without importing.
pub(crate) fn content_search(config: &ServerConfig) -> content_cache::ContentSearch {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let environment = std::env::var_os(content_cache::CONTENT_CACHE_ENV).map(PathBuf::from);
    let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    content_cache::ContentSearch::from_config(
        config.data.vanilla_data_dir.as_deref(),
        environment,
        home.as_deref(),
        &working_dir,
    )
}

async fn content_command(config_path: &Path, command: ContentCommand) -> Result<()> {
    let configured_dir = match std::fs::metadata(config_path) {
        Ok(_) => load_config(config_path)?.data.vanilla_data_dir,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("reading config file {}", config_path.display()));
        }
    };
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let environment = std::env::var_os(content_cache::CONTENT_CACHE_ENV).map(PathBuf::from);
    let working_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match command {
        ContentCommand::Import {
            version,
            from,
            download: _,
            cache,
        } => {
            let mut search = content_cache::ContentSearch::from_config(
                cache.as_deref().or(configured_dir.as_deref()),
                environment,
                home.as_deref(),
                &working_dir,
            );
            if let Some(cache) = cache {
                search.explicit = Some(cache);
            }
            let request = content_import::ImportRequest {
                version,
                source: match from {
                    Some(jar) => content_import::ContentSource::LocalJar(jar),
                    None => content_import::ContentSource::Download,
                },
                cache: search.import_target(),
            };
            let report = content_import::import_content(&request).await?;
            println!(
                "imported vanilla content {} into {}: {} registries, {} entries",
                request.version,
                report.cache.display(),
                report.registries,
                report.entries,
            );
            Ok(())
        }
    }
}

fn manage_operators(config_path: &Path, command: OperatorCommand) -> Result<()> {
    let mut config = load_config(config_path)?;
    if config.admin.operators_file.is_none() {
        config.admin.operators_file = Some(PathBuf::from("ops.json"));
    }
    let operation = match command {
        OperatorCommand::Add { identity } => OperatorFileOperation::Add(identity),
        OperatorCommand::Remove { identity } => OperatorFileOperation::Remove(identity),
        OperatorCommand::List => OperatorFileOperation::List,
    };
    let result = config.manage_operator_file(config_path, operation.clone())?;
    match operation {
        OperatorFileOperation::Add(identity) => println!(
            "operator {} {}",
            identity.trim(),
            if result.changed {
                "added"
            } else {
                "already present"
            }
        ),
        OperatorFileOperation::Remove(identity) => println!(
            "{} operator {}",
            if result.changed {
                "removed"
            } else {
                "not found:"
            },
            identity.trim()
        ),
        OperatorFileOperation::List => {
            for identity in result.identities {
                println!("{identity}");
            }
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let output = console::ConsoleOutput::new();
    let pregenerate = match &cli.command {
        Some(Command::Pregenerate { from, to }) => Some((*from, *to)),
        _ => None,
    };
    if cli.check && pregenerate.is_some() {
        eprintln!("error: --check cannot be combined with the pregenerate subcommand");
        return ExitCode::FAILURE;
    }
    let terminal_console = (!cli.check && cli.command.is_none()).then(|| {
        (
            output.clone(),
            !cli.no_console && console::ConsoleOutput::supported(),
        )
    });
    let result = match (cli.check, cli.command) {
        (true, None) => check_config(&cli.config),
        (false, Some(Command::Pregenerate { .. })) | (false, None) => match init_tracing() {
            Ok(warning_ring) => {
                serve(&cli.config, warning_ring, terminal_console, pregenerate).await
            }
            Err(error) => Err(error),
        },
        (false, Some(Command::Operator { command })) => manage_operators(&cli.config, command),
        (false, Some(Command::Content { command })) => content_command(&cli.config, command).await,
        (true, Some(_)) => Err(anyhow::anyhow!(
            "--check cannot be combined with a subcommand"
        )),
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
#[path = "structure_rules_tests.rs"]
mod structure_rules_tests;

#[cfg(test)]
#[path = "component_startup_tests.rs"]
mod component_startup_tests;

#[cfg(test)]
#[path = "main_tests.rs"]
mod main_tests;
