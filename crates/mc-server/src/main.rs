//! `mc-server` binary entry point.

use std::collections::{BTreeMap, HashMap};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;
use mc_server::ServerConfig;

mod startup_validation;

#[cfg(test)]
use startup_validation::{PersistedWorldContract, WORLD_CONTRACT_SCHEMA, world_contract_path};
use startup_validation::{
    WorldSource, ensure_world_contract, has_non_directory_ancestor, is_public_bind_ip,
    required_world_dir, validate_runtime_config, validate_vanilla_sidecar_version,
    world_region_root_is_blocked,
};

const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(6);
const STARTUP_LIGHT_BAKE_WORKER_CAP: usize = 16;
const STARTUP_GENERATION_QUEUE_BATCHES: usize = 8;

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
    let _ip: IpAddr = cfg.network.bind_address.parse().with_context(|| {
        format!(
            "validating network.bind_address `{}`",
            cfg.network.bind_address
        )
    })?;
    validate_runtime_config(&cfg)?;
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
    operator_warnings: Vec<OperatorWarning>,
}

impl<'a> From<&'a ServerConfig> for EffectiveConfig<'a> {
    fn from(config: &'a ServerConfig) -> Self {
        Self {
            config,
            effective_chunk_pipeline: EffectiveChunkPipeline::from(
                config.chunk_pipeline.to_network(),
            ),
            effective_autoscale: EffectiveAutoscale::from(config),
            operator_warnings: operator_warnings(config),
        }
    }
}

#[derive(serde::Serialize)]
struct OperatorWarning {
    code: &'static str,
    message: &'static str,
}

#[derive(serde::Deserialize)]
struct VanillaVersionMetadata {
    id: String,
    world_version: u32,
    protocol_version: i32,
}

fn operator_warnings(config: &ServerConfig) -> Vec<OperatorWarning> {
    let mut warnings = Vec::new();
    match &config.data.world_dir {
        Some(world_dir) => {
            match std::fs::metadata(world_dir) {
                Ok(metadata) => {
                    if !metadata.is_dir() {
                        warnings.push(OperatorWarning {
                            code: "world_dir_not_directory",
                            message: "[data].world_dir exists but is not a directory; check and serve reject this configuration",
                        });
                    } else if world_region_root_is_blocked(world_dir) {
                        warnings.push(OperatorWarning {
                            code: "world_region_not_directory",
                            message: "[data].world_dir/region exists but is not a directory, and no modern overworld region directory exists; check and serve reject this configuration",
                        });
                    }
                }
                Err(_) if has_non_directory_ancestor(world_dir) => {
                    warnings.push(OperatorWarning {
                        code: "world_dir_parent_not_directory",
                        message: "[data].world_dir has a non-directory parent path; check and serve reject this configuration",
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    warnings.push(OperatorWarning {
                        code: "world_dir_missing_on_disk",
                        message: "[data].world_dir does not exist on disk; serve will create a fresh world directory",
                    });
                }
                Err(_) => warnings.push(OperatorWarning {
                    code: "world_dir_metadata_unavailable",
                    message: "[data].world_dir metadata is unavailable; check and serve reject this configuration",
                }),
            }
        }
        None => {
            warnings.push(OperatorWarning {
                code: "missing_world_dir",
                message: "no [data].world_dir configured; check and serve reject this configuration",
            });
        }
    }

    if let Some(vanilla_data_dir) = &config.data.vanilla_data_dir {
        match std::fs::metadata(vanilla_data_dir) {
            Ok(metadata) if metadata.is_dir() => {
                let version_path = vanilla_data_dir.join("version.json");
                let mut current_version = false;
                match std::fs::read_to_string(version_path) {
                    Ok(raw) => match serde_json::from_str::<VanillaVersionMetadata>(&raw) {
                        Ok(version) => {
                            let release_matches = version.id == mc_protocol::TARGET_RELEASE;
                            let world_version_matches =
                                version.world_version == mc_protocol::WORLD_VERSION;
                            let protocol_matches =
                                version.protocol_version == mc_protocol::PROTOCOL_VERSION;
                            current_version =
                                release_matches && world_version_matches && protocol_matches;
                            if !release_matches {
                                warnings.push(OperatorWarning {
                                    code: "vanilla_data_release_mismatch",
                                    message: "data.vanilla_data_dir version.json id does not match Solaris target release; rerun tools/extract-vanilla-data.sh for the target vanilla jar",
                                });
                            }
                            if !world_version_matches {
                                warnings.push(OperatorWarning {
                                    code: "vanilla_data_world_version_mismatch",
                                    message: "data.vanilla_data_dir version.json world_version does not match Solaris world version; rerun tools/extract-vanilla-data.sh for the target vanilla jar",
                                });
                            }
                            if !protocol_matches {
                                warnings.push(OperatorWarning {
                                    code: "vanilla_data_protocol_mismatch",
                                    message: "data.vanilla_data_dir version.json protocol_version does not match Solaris; rerun tools/extract-vanilla-data.sh for the target vanilla jar",
                                });
                            }
                        }
                        Err(_) => warnings.push(OperatorWarning {
                            code: "vanilla_data_version_invalid",
                            message: "data.vanilla_data_dir version.json is not readable as UTF-8, is not valid metadata, or is missing id, world_version, or protocol_version; rerun tools/extract-vanilla-data.sh for the target vanilla jar",
                        }),
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        warnings.push(OperatorWarning {
                            code: "vanilla_data_version_missing",
                            message: "data.vanilla_data_dir is missing version.json; rerun tools/extract-vanilla-data.sh for the target vanilla jar",
                        });
                    }
                    Err(_) => warnings.push(OperatorWarning {
                        code: "vanilla_data_version_invalid",
                        message: "data.vanilla_data_dir version.json is not readable as UTF-8, is not valid metadata, or is missing id, world_version, or protocol_version; rerun tools/extract-vanilla-data.sh for the target vanilla jar",
                    }),
                }
                if current_version {
                    if !vanilla_registry_tree_is_complete(vanilla_data_dir) {
                        warnings.push(OperatorWarning {
                            code: "vanilla_data_registry_tree_incomplete",
                            message: "data.vanilla_data_dir is missing required registry JSON under data/minecraft; rerun tools/extract-vanilla-data.sh for the target vanilla jar",
                        });
                    } else if !vanilla_block_light_report_matches_target(vanilla_data_dir) {
                        warnings.push(OperatorWarning {
                            code: "vanilla_data_block_light_report_invalid",
                            message: "data.vanilla_data_dir reports/block_light.json is missing, malformed, or targets a different release; rerun tools/extract-vanilla-data.sh for the target vanilla jar",
                        });
                    } else if !vanilla_tags_are_usable(vanilla_data_dir) {
                        warnings.push(OperatorWarning {
                            code: "vanilla_data_tags_unavailable",
                            message: "data.vanilla_data_dir tags are missing, malformed, or lack required resolved block/item/entity_type entries; rerun tools/extract-vanilla-data.sh for the target vanilla jar",
                        });
                    } else if !vanilla_recipes_are_usable(vanilla_data_dir) {
                        warnings.push(OperatorWarning {
                            code: "vanilla_data_recipes_unavailable",
                            message: "data.vanilla_data_dir recipes are missing, malformed, or contain no supported shaped, shapeless, smelting, blasting, smoking, or campfire cooking entries; rerun tools/extract-vanilla-data.sh for the target vanilla jar",
                        });
                    } else if !vanilla_loot_is_usable(vanilla_data_dir) {
                        warnings.push(OperatorWarning {
                            code: "vanilla_data_loot_unavailable",
                            message: "data.vanilla_data_dir loot tables are missing, malformed, or contain no supported simple block/entity drops; rerun tools/extract-vanilla-data.sh for the target vanilla jar",
                        });
                    }
                }
            }
            Ok(_) => warnings.push(OperatorWarning {
                code: "vanilla_data_dir_not_directory",
                message: "data.vanilla_data_dir exists but is not a directory; rerun tools/extract-vanilla-data.sh or remove data.vanilla_data_dir to use embedded fallback data",
            }),
            Err(_) if has_non_directory_ancestor(vanilla_data_dir) => {
                warnings.push(OperatorWarning {
                    code: "vanilla_data_dir_parent_not_directory",
                    message: "data.vanilla_data_dir has a non-directory parent path; rerun tools/extract-vanilla-data.sh or remove data.vanilla_data_dir to use embedded fallback data",
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                warnings.push(OperatorWarning {
                    code: "vanilla_data_dir_missing_on_disk",
                    message: "data.vanilla_data_dir does not exist on disk; rerun tools/extract-vanilla-data.sh or remove data.vanilla_data_dir to use embedded fallback data",
                });
            }
            Err(_) => warnings.push(OperatorWarning {
                code: "vanilla_data_dir_metadata_unavailable",
                message: "data.vanilla_data_dir metadata is unavailable; serve may fail to load authoritative sidecar data or remove data.vanilla_data_dir to use embedded fallback data",
            }),
        }
    }

    if config
        .admin
        .operators
        .iter()
        .any(|operator| operator.trim().is_empty())
    {
        warnings.push(OperatorWarning {
            code: "admin_operator_entry_blank",
            message: "admin.operators contains an empty or whitespace-only name; blank entries never grant operator permissions",
        });
    }

    if config
        .auth
        .whitelist
        .iter()
        .any(|entry| entry.trim().is_empty())
    {
        warnings.push(OperatorWarning {
            code: "auth_whitelist_entry_blank",
            message: "auth.whitelist contains an empty or whitespace-only entry; blank entries never allow login",
        });
    }
    if config
        .auth
        .banned_players
        .iter()
        .any(|entry| entry.trim().is_empty())
    {
        warnings.push(OperatorWarning {
            code: "auth_banned_player_entry_blank",
            message: "auth.banned_players contains an empty or whitespace-only entry; blank entries never deny login",
        });
    }

    let Some(ip) = config.network.bind_address.parse::<IpAddr>().ok() else {
        return warnings;
    };
    if !is_public_bind_ip(ip) {
        return warnings;
    }

    if config.admin.allow_local_dev_operators {
        warnings.push(OperatorWarning {
            code: "public_bind_local_dev_operators",
            message: "allow_local_dev_operators cannot be enabled on a public bind address; serve will fail",
        });
    }
    if !config.auth.online_mode {
        warnings.push(OperatorWarning {
            code: "public_bind_offline_mode",
            message: "offline-mode Solaris authentication cannot be used on a public bind address; serve will fail",
        });
    }
    warnings
}

fn vanilla_registry_tree_is_complete(vanilla_data_dir: &Path) -> bool {
    let minecraft_root = vanilla_data_dir.join("data").join("minecraft");
    minecraft_root.is_dir()
        && mc_data::KNOWN_REGISTRIES
            .iter()
            .all(|(_, fs_subpath)| registry_dir_has_json(&minecraft_root.join(fs_subpath)))
}

fn registry_dir_has_json(path: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            return false;
        };
        if file_type.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "json")
        {
            return true;
        }
        if file_type.is_dir() && registry_dir_has_json(&path) {
            return true;
        }
    }
    false
}

fn vanilla_block_light_report_matches_target(vanilla_data_dir: &Path) -> bool {
    let path = vanilla_data_dir.join("reports").join("block_light.json");
    mc_data::block_light::load(path).is_ok_and(|table| table.version == mc_protocol::TARGET_RELEASE)
}

fn vanilla_tags_are_usable(vanilla_data_dir: &Path) -> bool {
    let Ok(protocol_data) = load_effective_protocol_data(Some(vanilla_data_dir)) else {
        return false;
    };
    let items = mc_data::items::solaris_required_items();
    let blocks = mc_data::blocks::solaris_required_blocks_report();
    load_effective_tags(Some(vanilla_data_dir), &protocol_data.data, &items, &blocks).is_ok()
}

fn vanilla_recipes_are_usable(vanilla_data_dir: &Path) -> bool {
    load_effective_recipes(Some(vanilla_data_dir)).is_ok()
}

fn vanilla_loot_is_usable(vanilla_data_dir: &Path) -> bool {
    load_effective_loot(Some(vanilla_data_dir)).is_ok()
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
            runtime_mode: if config.autoscale.enabled {
                "live_adaptive_work_budgets"
            } else {
                "disabled"
            },
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
            memory_pressure_percent: policy.memory_pressure_percent,
            scale_down_after_ticks: policy.scale_down_after_ticks,
            scale_up_after_ticks: policy.scale_up_after_ticks,
        }
    }
}

async fn serve(path: &Path) -> Result<()> {
    let cfg = load_config(path)?;
    validate_runtime_config(&cfg)?;
    let configured_geometry = cfg.data.chunk_geometry().map_err(anyhow::Error::msg)?;
    let world_dir = required_world_dir(&cfg)?;
    let worldgen_mode = cfg.data.worldgen_mode.to_worldgen();
    let mut prepared_plugins = if let Some(directory) = cfg.plugins.directory.as_deref() {
        Some(
            mc_script::prepare_lua_plugins(mc_script::LuaHostConfig::new(directory))
                .with_context(|| format!("preparing Lua plugins from {}", directory.display()))?,
        )
    } else {
        None
    };
    let plugin_ore_profile = prepared_plugins
        .as_ref()
        .and_then(mc_script::PreparedLuaPlugins::worldgen_ore_profile);
    let plugin_settlement_plan = prepared_plugins
        .as_ref()
        .and_then(mc_script::PreparedLuaPlugins::worldgen_settlement_plan)
        .cloned();
    let ore_profile_name = plugin_ore_profile
        .map(mc_script::LuaWorldgenOreProfile::contract_name)
        .unwrap_or("vanilla");
    let settlement_contract = plugin_settlement_plan
        .as_ref()
        .map(mc_script::LuaSettlementPlan::contract_name)
        .unwrap_or_else(|| "vanilla".to_owned());
    let world_source = ensure_world_contract(
        world_dir,
        configured_geometry,
        cfg.data.seed,
        worldgen_mode.contract_name(),
        ore_profile_name,
        &settlement_contract,
    )?;

    let protocol_data = load_effective_protocol_data(cfg.data.vanilla_data_dir.as_deref())?;
    let data = protocol_data.data;
    tracing::info!(
        registries = data.registry_count(),
        entries = data.entry_count(),
        source = protocol_data.source,
        "registry index loaded",
    );

    let loader_manifest = prepared_plugins
        .as_ref()
        .map(|plugins| mc_net::LoaderManifest::from_script_bundles(plugins.client_bundles()))
        .transpose()
        .context("reading Solaris Loader artifact identities")?
        .filter(|manifest| !manifest.is_empty())
        .map(|manifest| {
            manifest
                .encode()
                .context("encoding aggregated Solaris Loader manifest")?;
            Ok::<_, anyhow::Error>(Arc::new(manifest))
        })
        .transpose()?;
    let mut blocks_report = mc_data::blocks::solaris_required_blocks_report();
    let block_light_source =
        load_effective_block_light(cfg.data.vanilla_data_dir.as_deref(), &blocks_report)?;
    let mut block_light = block_light_source.table;
    tracing::info!(
        version = %block_light.version,
        states = block_light.len(),
        source = block_light_source.source,
        "block-light table loaded",
    );
    let block_mining_source =
        load_effective_block_mining(cfg.data.vanilla_data_dir.as_deref(), &blocks_report)?;
    tracing::info!(
        states = block_mining_source
            .table
            .as_ref()
            .map_or(0, |table| table.len()),
        source = block_mining_source.source,
        "block-mining table loaded",
    );
    if let Some(manifest) = loader_manifest.as_deref() {
        for state_id in manifest
            .append_world_block_report(&mut blocks_report)
            .context("registering Solaris Loader world blocks")?
        {
            anyhow::ensure!(
                usize::try_from(state_id).ok() == Some(block_light.len()),
                "Solaris Loader block state must follow the validated light table"
            );
            block_light.append_opaque_state();
        }
    }
    let block_light = Arc::new(block_light);
    let block_states: usize = blocks_report.iter().map(|b| b.states.len()).sum();
    let blocks = Arc::new(
        mc_world::BlockRegistry::from_report(&blocks_report)
            .context("building block-state registry from embedded JSON")?,
    );
    tracing::info!(
        blocks = blocks_report.len(),
        states = block_states,
        "server block registry loaded",
    );
    let block_explosion_source =
        load_effective_block_explosion(cfg.data.vanilla_data_dir.as_deref())?;
    tracing::info!(
        states = block_explosion_source
            .table
            .as_ref()
            .map_or(0, |table| table.len()),
        source = block_explosion_source.source,
        "block-explosion table loaded",
    );
    let items = Arc::new(mc_data::items::solaris_required_items());
    tracing::info!(entries = items.len(), "embedded item registry loaded");
    let structure_rules = structure_rules_for_startup(
        cfg.data.seed,
        cfg.data.worldgen_mode,
        cfg.data.vanilla_data_dir.as_deref(),
        &blocks,
        &items,
        plugin_settlement_plan.as_ref(),
    )?;
    let chunk_pipeline = cfg.chunk_pipeline.to_network();
    let terrain_generator = build_terrain_generator(
        cfg.data.seed,
        worldgen_mode,
        configured_geometry,
        Arc::clone(&blocks),
        structure_rules,
        plugin_ore_profile,
    )?;
    let item_facts_source = load_effective_item_facts(cfg.data.vanilla_data_dir.as_deref())?;
    let item_facts = Arc::new(item_facts_source.table);
    tracing::info!(
        entries = item_facts.len(),
        source = item_facts_source.source,
        "item component facts loaded"
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
                // Solaris worlds generate missing chunks. Imported vanilla
                // worlds stay read-only with respect to terrain authority.
                let startup_workers =
                    startup_chunk_worker_threads(chunk_pipeline.chunk_worker_threads);
                let startup_light_workers = startup_light_bake_worker_threads(startup_workers);
                let mut storage = storage.with_item_registry(Arc::clone(&items));
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
                        cfg.server.view_distance,
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
                        "empty world pre-generated around spawn; disk flush queued for startup dirty checkpoint",
                    );
                } else {
                    let prepared = prepare_existing_spawn_window(
                        &mut storage,
                        block_light.as_ref(),
                        cfg.server.view_distance,
                        startup_light_workers,
                    )?;
                    tracing::info!(
                        path = %world_dir.display(),
                        chunks = prepared.warmed,
                        baked = prepared.baked,
                        flushed = 0usize,
                        dirty = prepared.dirty,
                        view_distance = cfg.server.view_distance,
                        "existing world spawn window warmed",
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

    let data = Arc::new(data);
    let tag_source = load_effective_tags(
        cfg.data.vanilla_data_dir.as_deref(),
        &data,
        &items,
        &blocks_report,
    )?;
    let tags = Arc::new(tag_source.tags);
    tracing::info!(
        tags = tags.total_tags(),
        entries = tags.total_entries(),
        source = tag_source.source,
        "tags loaded"
    );
    let recipe_source = load_effective_recipes(cfg.data.vanilla_data_dir.as_deref())?;
    let recipes = Arc::new(recipe_source.recipes);
    tracing::info!(
        entries = recipes.len(),
        source = recipe_source.source,
        "recipe registry loaded"
    );
    let loot_source = load_effective_loot(cfg.data.vanilla_data_dir.as_deref())?;
    let loot = Arc::new(loot_source.tables);
    tracing::info!(
        drops = loot.total_drops(),
        source = loot_source.source,
        "survival loot tables loaded"
    );

    let mut block_facts = mc_data::block_facts::BlockFactsTable::from_blocks_report_with_mining(
        &blocks_report,
        block_mining_source.table.as_ref(),
    );
    if let Some(table) = block_explosion_source.table {
        block_facts = block_facts.with_explosion_table(table);
    }
    let block_facts = Arc::new(block_facts);
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
    let (bound, lua_host) = if let Some(prepared) = prepared_plugins.take() {
        let directory = cfg
            .plugins
            .directory
            .as_deref()
            .expect("prepared plugins have a configured directory");
        let (boundary, host) = mc_script::start_prepared_lua_host(prepared)
            .with_context(|| format!("starting Lua plugins from {}", directory.display()))?;
        tracing::info!(
            directory = %directory.display(),
            loaded = host.loaded_plugins(),
            "Lua plugin host started"
        );
        match mc_net::bind_with_scripts(net, boundary).await {
            Ok(bound) => (bound, Some(host)),
            Err(error) => {
                join_lua_host(host).await?;
                return Err(error).context("network bind");
            }
        }
    } else {
        (mc_net::bind(net).await.context("network bind")?, None)
    };
    let result = run_bound_server(bound, shutdown_handle).await;
    if let Some(host) = lua_host {
        join_lua_host(host).await?;
    }
    result
}

async fn run_bound_server(
    bound: mc_net::BoundServer,
    shutdown_handle: mc_net::ShutdownHandle,
) -> Result<()> {
    // Every exit path drains admitted work and performs exactly one final save.
    // Ctrl-C only requests shutdown; it then waits for that same lifecycle.
    let mut run_fut = std::pin::pin!(bound.serve_and_save());
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
            shutdown_handle.request();
            match tokio::time::timeout(SHUTDOWN_DRAIN_TIMEOUT, run_fut.as_mut()).await {
                Ok(result) => result.context("network listener"),
                Err(_) => anyhow::bail!("shutdown drain and final save timed out"),
            }
        }
    }
}

async fn join_lua_host(host: mc_script::LuaHost) -> Result<()> {
    let result = tokio::task::spawn_blocking(move || host.join())
        .await
        .context("joining Lua host task")?;
    if result.is_err() {
        bail!("Lua host thread panicked");
    }
    Ok(())
}

fn build_terrain_generator(
    seed: i64,
    worldgen_mode: mc_worldgen::WorldgenMode,
    geometry: mc_world::ChunkGeometry,
    blocks: Arc<mc_world::BlockRegistry>,
    structure_rules: mc_worldgen::StructureRules,
    ore_profile: Option<mc_script::LuaWorldgenOreProfile>,
) -> Result<Arc<mc_worldgen::TerrainGenerator>> {
    let biomes = mc_worldgen::BiomeRules::vanilla_overworld();
    let mut generator =
        mc_worldgen::TerrainGenerator::try_with_biome_rules(seed, Arc::clone(&blocks), biomes)
            .context("building terrain generator")?
            .with_geometry(geometry)
            .with_mode(worldgen_mode)
            .with_structures(structure_rules);
    if matches!(
        ore_profile,
        Some(mc_script::LuaWorldgenOreProfile::GeologicalDeposits)
    ) {
        generator = generator.with_geological_deposits(blocks.as_ref());
    }
    Ok(Arc::new(generator))
}

fn structure_rules_for_startup(
    seed: i64,
    worldgen_mode: mc_server::WorldgenMode,
    vanilla_data_dir: Option<&Path>,
    blocks: &mc_world::BlockRegistry,
    items: &mc_data::items::ItemRegistry,
    settlement_plan: Option<&mc_script::LuaSettlementPlan>,
) -> Result<mc_worldgen::StructureRules> {
    if let Some(settlement_plan) = settlement_plan {
        let vanilla_data_dir = vanilla_data_dir.context(
            "worldgen settlement profile plains_village_prototype requires data.vanilla_data_dir",
        )?;
        let parts = settlement_plan
            .buildings()
            .iter()
            .map(|building| match building.template() {
                mc_script::LuaSettlementBuildingTemplate::PlainsFountain => {
                    Ok(mc_worldgen::PlainsVillagePrototypePart::Fountain)
                }
                mc_script::LuaSettlementBuildingTemplate::PlainsSmallHouse => {
                    Ok(mc_worldgen::PlainsVillagePrototypePart::SmallHouse)
                }
                mc_script::LuaSettlementBuildingTemplate::PlainsToolsmith => {
                    Ok(mc_worldgen::PlainsVillagePrototypePart::Toolsmith)
                }
                _ => anyhow::bail!("unsupported settlement building template"),
            })
            .collect::<Result<Vec<_>>>()?;
        let inhabitants = settlement_plan
            .inhabitants()
            .iter()
            .map(|inhabitant| {
                let entity_type = match inhabitant.kind() {
                    mc_script::LuaSettlementInhabitantKind::Villager => "minecraft:villager",
                    _ => anyhow::bail!("unsupported settlement inhabitant kind"),
                };
                let profession = match inhabitant.job() {
                    mc_script::LuaSettlementJob::Unemployed => "none",
                    mc_script::LuaSettlementJob::Toolsmith => "toolsmith",
                    _ => anyhow::bail!("unsupported settlement inhabitant job"),
                };
                Ok(mc_worldgen::StructureInhabitant {
                    id: format!("{}:{}", settlement_plan.owner_plugin_id(), inhabitant.id()),
                    entity_type: entity_type.to_owned(),
                    villager_kind: "plains".to_owned(),
                    profession: profession.to_owned(),
                    level: 1,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let rules = mc_worldgen::StructureRules::plains_village_prototype_with_plan(
            vanilla_data_dir,
            blocks,
            &parts,
            inhabitants,
        )
        .context("loading plains village prototype from vanilla structure data")?;
        tracing::info!(
            owner = settlement_plan.owner_plugin_id(),
            buildings = settlement_plan.buildings().len(),
            inhabitants = settlement_plan.inhabitants().len(),
            extensions = settlement_plan.extensions().len(),
            "materialized plugin settlement plan",
        );
        return Ok(if seed == 0 {
            rules.with_fixed_center((72, 8))
        } else {
            rules
        });
    }
    if seed == 0 && worldgen_mode == mc_server::WorldgenMode::VanillaLike {
        return mc_worldgen::StructureRules::solaris_playable_ruin(blocks, items)
            .context("resolving Solaris playable ruin");
    }
    Ok(mc_worldgen::StructureRules::none())
}

fn chunk_cache_size_for_view_distance(view_distance: i32) -> usize {
    let view_distance = view_distance.max(0) as usize;
    let radius = if view_distance == 0 {
        1
    } else {
        view_distance + 2
    };
    let width = radius * 2 + 1;
    width * width
}

fn startup_chunk_worker_threads(configured_workers: usize) -> usize {
    let available = std::thread::available_parallelism()
        .map(std::num::NonZeroUsize::get)
        .unwrap_or(1);
    configured_workers.max(available).max(1)
}

fn startup_light_bake_worker_threads(chunk_workers: usize) -> usize {
    chunk_workers
        .saturating_mul(2)
        .clamp(1, STARTUP_LIGHT_BAKE_WORKER_CAP)
}

fn generate_spawn_window(
    storage: &mut mc_world::WorldStorage,
    generator: Arc<dyn mc_world::ChunkGenerator>,
    view_distance: i32,
    worker_threads: usize,
    light_bake_worker_threads: usize,
    block_light: Option<&mc_data::block_light::BlockLightTable>,
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
    let queue_batches = workers.clamp(1, STARTUP_GENERATION_QUEUE_BATCHES);
    let (tx, rx) = std::sync::mpsc::sync_channel(queue_batches);
    let batch_size = 8usize.min(total);
    let started = Instant::now();
    let log_every = (total / 20).max(64);
    let generated = std::thread::scope(|scope| -> Result<usize> {
        let mut handles = Vec::with_capacity(workers);
        for worker_index in 0..workers {
            let positions = Arc::clone(&positions);
            let next = Arc::clone(&next);
            let tx = tx.clone();
            let generator = Arc::clone(&generator);
            let handle = std::thread::Builder::new()
                .name(format!("solaris-spawn-gen-{worker_index}"))
                .spawn_scoped(scope, move || {
                    let mut batch = Vec::with_capacity(batch_size);
                    loop {
                        let idx = next.fetch_add(1, Ordering::Relaxed);
                        let Some(&pos) = positions.get(idx) else {
                            break;
                        };
                        let chunk = generator.generate(pos);
                        batch.push((pos, chunk));
                        if batch.len() >= batch_size && tx.send(std::mem::take(&mut batch)).is_err()
                        {
                            break;
                        }
                    }
                    if !batch.is_empty() {
                        let _ = tx.send(batch);
                    }
                })
                .with_context(|| format!("spawning worldgen worker {worker_index}"))?;
            handles.push(handle);
        }
        drop(tx);

        let mut generated = 0usize;
        let mut last_log = Instant::now();
        let mut consumer_error = None;
        'receive: while let Ok(batch) = rx.recv() {
            for (pos, chunk) in batch {
                if storage.stats().dirty_chunk_cache_saturated
                    && let Err(error) = storage.flush_dirty().with_context(|| {
                        format!(
                            "flushing dirty chunks before pre-generating spawn chunk ({}, {})",
                            pos.x, pos.z
                        )
                    })
                {
                    consumer_error = Some(error);
                    break 'receive;
                }
                if let Err(error) = storage
                    .insert_generated_chunk(pos, chunk)
                    .with_context(|| format!("pre-generating spawn chunk ({}, {})", pos.x, pos.z))
                {
                    consumer_error = Some(error);
                    break 'receive;
                }
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
        drop(rx);

        let worker_panicked = handles
            .into_iter()
            .fold(false, |panicked, handle| handle.join().is_err() || panicked);
        if let Some(error) = consumer_error {
            return Err(error);
        }
        if worker_panicked {
            bail!("spawn pre-generation worker panicked");
        }
        if generated != total {
            bail!("spawn pre-generation incomplete: generated {generated} of {total} chunks");
        }
        Ok(generated)
    })?;

    let elapsed = started.elapsed();
    let chunks_per_second = generated as f64 / elapsed.as_secs_f64().max(0.001);
    tracing::info!(
        generated,
        total,
        elapsed_ms = elapsed.as_millis(),
        chunks_per_second,
        "empty world pre-generation finished",
    );
    if let Some(block_light) = block_light {
        let baked = bake_spawn_window_light(
            storage,
            block_light,
            view_distance,
            light_bake_worker_threads,
        )?;
        tracing::info!(
            baked,
            elapsed_ms = started.elapsed().as_millis(),
            "empty world startup light bake finished",
        );
    }
    Ok(generated)
}

fn warm_spawn_window(storage: &mut mc_world::WorldStorage, view_distance: i32) -> Result<usize> {
    let mut warmed = 0usize;
    for pos in spawn_window_positions(view_distance) {
        if storage
            .get_chunk(pos)
            .with_context(|| format!("warming spawn chunk ({}, {})", pos.x, pos.z))?
            .is_some()
        {
            warmed += 1;
        }
    }
    Ok(warmed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ExistingSpawnWindowPrep {
    warmed: usize,
    baked: usize,
    dirty: usize,
}

fn prepare_existing_spawn_window(
    storage: &mut mc_world::WorldStorage,
    block_light: &mc_data::block_light::BlockLightTable,
    view_distance: i32,
    worker_threads: usize,
) -> Result<ExistingSpawnWindowPrep> {
    let warmed = warm_spawn_window(storage, view_distance)?;
    let baked =
        bake_missing_spawn_window_light(storage, block_light, view_distance, worker_threads)?;
    let dirty = storage.dirty_count();
    if dirty > 0 {
        tracing::info!("Preparing world... 95% (warmed spawn window resident)");
    }
    Ok(ExistingSpawnWindowPrep {
        warmed,
        baked,
        dirty,
    })
}

fn bake_spawn_window_light(
    storage: &mut mc_world::WorldStorage,
    block_light: &mc_data::block_light::BlockLightTable,
    view_distance: i32,
    worker_threads: usize,
) -> Result<usize> {
    let positions = spawn_view_positions(view_distance);
    bake_spawn_window_light_for_positions(
        storage,
        block_light,
        view_distance,
        worker_threads,
        positions,
    )
}

fn bake_missing_spawn_window_light(
    storage: &mut mc_world::WorldStorage,
    block_light: &mc_data::block_light::BlockLightTable,
    view_distance: i32,
    worker_threads: usize,
) -> Result<usize> {
    let mut missing = Vec::new();
    for pos in spawn_view_positions(view_distance) {
        let Some(chunk) = storage.cached_chunk_snapshot(pos) else {
            bail!(
                "missing warmed spawn chunk ({}, {}) while checking baked light",
                pos.x,
                pos.z
            );
        };
        if mc_world::light::ChunkLight::from_chunk(&chunk).is_none() {
            missing.push(pos);
        }
    }
    bake_spawn_window_light_for_positions(
        storage,
        block_light,
        view_distance,
        worker_threads,
        missing,
    )
}

fn bake_spawn_window_light_for_positions(
    storage: &mut mc_world::WorldStorage,
    block_light: &mc_data::block_light::BlockLightTable,
    view_distance: i32,
    worker_threads: usize,
    positions: Vec<mc_world::ChunkPos>,
) -> Result<usize> {
    let total = positions.len();
    if total == 0 {
        return Ok(0);
    }
    let mut snapshots: HashMap<mc_world::ChunkPos, Arc<mc_world::Chunk>> = HashMap::new();
    for pos in spawn_window_positions(view_distance) {
        let Some(chunk) = storage.cached_chunk_snapshot(pos) else {
            bail!(
                "missing generated chunk ({}, {}) while baking spawn light",
                pos.x,
                pos.z
            );
        };
        snapshots.insert(pos, chunk);
    }

    let workers = worker_threads.max(1).min(total);
    tracing::info!(
        chunks = total,
        workers,
        view_distance,
        "spawn-window light bake started",
    );

    let positions = Arc::new(positions);
    let snapshots = Arc::new(snapshots);
    let next = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = std::sync::mpsc::channel::<
        Result<Vec<(mc_world::ChunkPos, mc_world::light::ChunkLight)>>,
    >();
    let batch_size = 8usize.min(total);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let positions = Arc::clone(&positions);
            let snapshots = Arc::clone(&snapshots);
            let next = Arc::clone(&next);
            let tx = tx.clone();
            scope.spawn(move || {
                let mut workspace = mc_world::light::LightWorkspace::new();
                let mut batch = Vec::with_capacity(batch_size);
                loop {
                    let idx = next.fetch_add(1, Ordering::Relaxed);
                    let Some(&pos) = positions.get(idx) else {
                        break;
                    };
                    let mut refs: [[Option<&mc_world::Chunk>; 3]; 3] = [[None; 3]; 3];
                    for dz in -1i32..=1 {
                        for dx in -1i32..=1 {
                            let neighbour = mc_world::ChunkPos {
                                x: pos.x + dx,
                                z: pos.z + dz,
                            };
                            refs[(dz + 1) as usize][(dx + 1) as usize] =
                                snapshots.get(&neighbour).map(|chunk| chunk.as_ref());
                        }
                    }
                    if refs[1][1].is_none() {
                        let _ = tx.send(Err(anyhow::anyhow!(
                            "missing centre chunk ({}, {}) while baking spawn light",
                            pos.x,
                            pos.z
                        )));
                        return;
                    }
                    let light =
                        mc_world::light::compute_chunk_light_in(&mut workspace, refs, block_light);
                    batch.push((pos, light));
                    if batch.len() >= batch_size && tx.send(Ok(std::mem::take(&mut batch))).is_err()
                    {
                        break;
                    }
                }
                if !batch.is_empty() {
                    let _ = tx.send(Ok(batch));
                }
            });
        }
        drop(tx);
        for batch in rx {
            for (pos, light) in batch? {
                if !storage.set_baked_light(pos, &light)? {
                    bail!(
                        "missing generated chunk ({}, {}) while storing baked spawn light",
                        pos.x,
                        pos.z
                    );
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    })?;

    Ok(total)
}

fn spawn_view_positions(view_distance: i32) -> Vec<mc_world::ChunkPos> {
    let radius = view_distance.max(0);
    let width = radius as usize * 2 + 1;
    let mut positions = Vec::with_capacity(width * width);
    for z in -radius..=radius {
        for x in -radius..=radius {
            positions.push(mc_world::ChunkPos { x, z });
        }
    }
    positions
}

fn spawn_window_positions(view_distance: i32) -> Vec<mc_world::ChunkPos> {
    let radius = view_distance.max(0) + 1;
    let width = radius as usize * 2 + 1;
    let mut positions = Vec::with_capacity(width * width);
    for z in -radius..=radius {
        for x in -radius..=radius {
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
        validate_vanilla_sidecar_version(vanilla_data_dir)?;
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
    blocks: &[mc_data::blocks::BlockReport],
) -> Result<EffectiveTags> {
    if let Some(vanilla_data_dir) = vanilla_data_dir {
        let tags = mc_data::tags::load(vanilla_data_dir, data)
            .with_context(|| format!("loading vanilla tags from {}", vanilla_data_dir.display()))?
            .with_vanilla_fuel_values(items);
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
        if !tags.fuel_values().matches_default_vanilla_26_1_2(items) {
            bail!(
                "vanilla tags from {} resolved {} furnace fuels instead of the canonical 26.1.2 default set; regenerate the sidecar",
                vanilla_data_dir.display(),
                tags.fuel_values().fuel_count(),
            );
        }
        return Ok(EffectiveTags {
            tags,
            source: "vanilla_sidecar",
        });
    }

    Ok(EffectiveTags {
        tags: mc_data::tags::solaris_required_client_tags(items, blocks),
        source: "embedded_solaris_fallback",
    })
}

fn load_effective_loot(vanilla_data_dir: Option<&Path>) -> Result<EffectiveLootTables> {
    if let Some(vanilla_data_dir) = vanilla_data_dir {
        let root = vanilla_data_dir
            .join("data")
            .join("minecraft")
            .join("loot_table");
        let mut tables = mc_data::loot::load_vanilla_subset(&root)
            .with_context(|| format!("loading vanilla loot tables from {}", root.display()))?;
        if tables.total_drops() > 0 {
            tables.fill_missing_from(mc_data::loot::builtin());
            tables.fill_missing_entity_items_from(mc_data::loot::builtin());
            return Ok(EffectiveLootTables {
                tables,
                source: "vanilla_sidecar_simple_subset+embedded_fallback",
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

struct EffectiveRecipes {
    recipes: Vec<mc_data::recipes::Recipe>,
    source: &'static str,
}

fn load_effective_recipes(vanilla_data_dir: Option<&Path>) -> Result<EffectiveRecipes> {
    if let Some(vanilla_data_dir) = vanilla_data_dir {
        let root = vanilla_data_dir
            .join("data")
            .join("minecraft")
            .join("recipe");
        let sidecar_recipes = mc_data::recipes::load_recipes(&root)
            .with_context(|| format!("loading vanilla recipes from {}", root.display()))?;
        if !sidecar_recipes.is_empty() {
            let mut sidecar_by_id: BTreeMap<_, _> = sidecar_recipes
                .into_iter()
                .map(|recipe| (recipe.id.clone(), recipe))
                .collect();
            let embedded = mc_data::recipes::solaris_required_recipes();
            let mut recipes = Vec::with_capacity(embedded.len() + sidecar_by_id.len());
            for fallback in embedded {
                let recipe = sidecar_by_id.remove(&fallback.id).unwrap_or(fallback);
                recipes.push(recipe);
            }
            recipes.extend(sidecar_by_id.into_values());
            return Ok(EffectiveRecipes {
                recipes,
                source: "vanilla_sidecar+stable_embedded_prefix",
            });
        }
        bail!(
            "vanilla recipes from {} had no supported recipes; run tools/extract-vanilla-data.sh with recipe data",
            root.display()
        );
    }

    Ok(EffectiveRecipes {
        recipes: mc_data::recipes::solaris_required_recipes(),
        source: "embedded_solaris_fallback",
    })
}

struct EffectiveBlockLight {
    table: mc_data::block_light::BlockLightTable,
    source: &'static str,
}

struct EffectiveItemFacts {
    table: mc_data::item_components::ItemFactsTable,
    source: &'static str,
}

struct EffectiveBlockMining {
    table: Option<mc_data::block_mining::BlockMiningTable>,
    source: &'static str,
}

struct EffectiveBlockExplosion {
    table: Option<mc_data::block_explosion::BlockExplosionTable>,
    source: &'static str,
}

fn load_effective_block_explosion(
    vanilla_data_dir: Option<&Path>,
) -> Result<EffectiveBlockExplosion> {
    let Some(vanilla_data_dir) = vanilla_data_dir else {
        return Ok(EffectiveBlockExplosion {
            table: None,
            source: "embedded_solaris_fallback",
        });
    };

    let path = vanilla_data_dir
        .join("reports")
        .join("block_explosion.json");
    let table =
        mc_data::block_explosion::load_block_explosion_report(&path).with_context(|| {
            format!(
                "loading vanilla block-explosion table from {}",
                path.display()
            )
        })?;
    Ok(EffectiveBlockExplosion {
        table: Some(table),
        source: "vanilla_sidecar",
    })
}

fn load_effective_block_mining(
    vanilla_data_dir: Option<&Path>,
    blocks_report: &[mc_data::blocks::BlockReport],
) -> Result<EffectiveBlockMining> {
    let Some(vanilla_data_dir) = vanilla_data_dir else {
        return Ok(EffectiveBlockMining {
            table: None,
            source: "embedded_solaris_fallback",
        });
    };

    let path = vanilla_data_dir.join("reports").join("block_mining.json");
    let table = mc_data::block_mining::load(&path)
        .with_context(|| format!("loading vanilla block-mining table from {}", path.display()))?;
    if let Some(max_state_id) = blocks_report
        .iter()
        .flat_map(|block| block.states.iter().map(|state| state.id as usize))
        .max()
        && table.len() <= max_state_id
    {
        bail!(
            "vanilla block-mining table from {} has {} states but blocks report requires state id {max_state_id}",
            path.display(),
            table.len()
        );
    }
    if table.version != mc_protocol::TARGET_RELEASE {
        bail!(
            "vanilla block-mining table from {} targets {} but Solaris targets {}",
            path.display(),
            table.version,
            mc_protocol::TARGET_RELEASE
        );
    }

    Ok(EffectiveBlockMining {
        table: Some(table),
        source: "vanilla_sidecar",
    })
}

fn load_effective_item_facts(vanilla_data_dir: Option<&Path>) -> Result<EffectiveItemFacts> {
    if let Some(vanilla_data_dir) = vanilla_data_dir {
        let path = vanilla_data_dir
            .join("reports")
            .join("minecraft")
            .join("components")
            .join("item");
        let table = mc_data::item_components::load_item_facts(&path).with_context(|| {
            format!(
                "loading vanilla item component facts from {}",
                path.display()
            )
        })?;
        if table.is_empty() {
            bail!(
                "vanilla item component facts from {} were empty; rerun tools/extract-vanilla-data.sh",
                path.display()
            );
        }
        return Ok(EffectiveItemFacts {
            table,
            source: "vanilla_sidecar",
        });
    }

    Ok(EffectiveItemFacts {
        table: mc_data::item_components::solaris_required_item_facts(),
        source: "embedded_solaris_fallback",
    })
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
    use std::collections::BTreeMap;

    use mc_data::Identifier;
    use serde_json::Value;

    fn terrain_registry_missing_grass_block() -> Arc<mc_world::BlockRegistry> {
        use mc_data::blocks::{BlockReport, BlockStateReport};
        let names = [
            "minecraft:air",
            "minecraft:bedrock",
            "minecraft:stone",
            "minecraft:dirt",
        ];
        let report = names
            .into_iter()
            .enumerate()
            .map(|(id, name)| BlockReport {
                id: Identifier::parse(name).unwrap(),
                properties: BTreeMap::new(),
                states: vec![BlockStateReport {
                    id: u32::try_from(id).unwrap(),
                    default: true,
                    properties: BTreeMap::new(),
                }],
            })
            .collect::<Vec<_>>();
        Arc::new(mc_world::BlockRegistry::from_report(&report).unwrap())
    }

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
    fn ensure_world_region_root_reports_blocked_legacy_region_file() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join("region");
        std::fs::write(&legacy, b"not a directory").unwrap();

        let error = ensure_world_region_root(tmp.path()).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("creating empty world region directory"),
            "{error:#}"
        );
        assert!(legacy.is_file());
    }

    #[test]
    fn chunk_cache_size_covers_view_plus_light_border() {
        assert_eq!(chunk_cache_size_for_view_distance(0), 9);
        assert_eq!(chunk_cache_size_for_view_distance(4), 169);
        assert_eq!(chunk_cache_size_for_view_distance(10), 625);
        assert_eq!(chunk_cache_size_for_view_distance(-1), 9);
    }

    #[test]
    fn runtime_config_rejects_invalid_chunk_geometry() {
        let world = tempfile::tempdir().unwrap();
        let toml_src = format!(
            r#"
                [server]
                name = "S"
                motd = "M"

                [network]
                bind_address = "127.0.0.1"
                port = 25565

                [data]
                world_dir = "{}"
                min_y = 1
                height = 255
            "#,
            world.path().display()
        );
        let config: ServerConfig = toml::from_str(&toml_src).unwrap();

        let error = validate_runtime_config(&config).unwrap_err();

        assert!(error.to_string().contains("data.min_y (1)"), "{error:#}");
        assert!(error.to_string().contains("data.height (255)"), "{error:#}");
    }

    #[test]
    fn world_contract_rejects_mismatched_geometry_before_world_open() {
        let world = tempfile::tempdir().unwrap();
        let original = mc_world::ChunkGeometry::new(0, 256).unwrap();
        let changed = mc_world::ChunkGeometry::new(-64, 384).unwrap();

        assert_eq!(
            ensure_world_contract(
                world.path(),
                original,
                7,
                "vanilla_like",
                "vanilla",
                "vanilla",
            )
            .unwrap(),
            WorldSource::SolarisGenerated,
        );
        let bytes = std::fs::read(world_contract_path(world.path())).unwrap();
        let persisted: PersistedWorldContract = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(persisted.schema, WORLD_CONTRACT_SCHEMA);
        assert_eq!(persisted.worldgen_revision, mc_worldgen::WORLDGEN_REVISION);
        assert_eq!(persisted.seed, 7);
        assert_eq!(persisted.mode, "vanilla_like");
        assert_eq!(persisted.ore_profile, "vanilla");
        assert_eq!(persisted.settlement_profile, "vanilla");
        assert_eq!(persisted.min_y, 0);
        assert_eq!(persisted.height, 256);

        let error = ensure_world_contract(
            world.path(),
            changed,
            7,
            "vanilla_like",
            "vanilla",
            "vanilla",
        )
        .unwrap_err();

        let message = error.to_string();
        assert!(message.contains("world contract geometry"), "{message}");
        assert!(message.contains("0..256"), "{message}");
        assert!(message.contains("-64..320"), "{message}");
    }

    #[test]
    fn world_contract_rejects_mismatched_worldgen_revision_before_world_open() {
        let world = tempfile::tempdir().unwrap();
        let geometry = mc_world::ChunkGeometry::new(-64, 384).unwrap();
        ensure_world_contract(
            world.path(),
            geometry,
            7,
            "vanilla_like",
            "vanilla",
            "vanilla",
        )
        .unwrap();

        let path = world_contract_path(world.path());
        let bytes = std::fs::read(&path).unwrap();
        let mut persisted: PersistedWorldContract = serde_json::from_slice(&bytes).unwrap();
        persisted.worldgen_revision = persisted.worldgen_revision.saturating_sub(1);
        std::fs::write(&path, serde_json::to_vec_pretty(&persisted).unwrap()).unwrap();

        let error = ensure_world_contract(
            world.path(),
            geometry,
            7,
            "vanilla_like",
            "vanilla",
            "vanilla",
        )
        .unwrap_err();
        assert!(error.to_string().contains("persisted worldgen revision="));
    }

    #[test]
    fn unversioned_anvil_world_opens_without_solaris_generation() {
        let world = tempfile::tempdir().unwrap();
        let region = world.path().join("region");
        std::fs::create_dir_all(&region).unwrap();
        std::fs::write(
            region.join("r.12.-7.mca"),
            b"not read during metadata preflight",
        )
        .unwrap();

        let geometry = mc_world::ChunkGeometry::new(0, 256).unwrap();
        assert_eq!(
            ensure_world_contract(
                world.path(),
                geometry,
                0,
                "vanilla_like",
                "vanilla",
                "vanilla",
            )
            .unwrap(),
            WorldSource::ExistingVanilla,
        );
        assert!(!world_contract_path(world.path()).exists());
    }

    #[test]
    fn unversioned_anvil_world_rejects_a_plugin_worldgen_profile() {
        let world = tempfile::tempdir().unwrap();
        let region = world.path().join("region");
        std::fs::create_dir_all(&region).unwrap();
        std::fs::write(region.join("r.0.0.mca"), b"not opened during preflight").unwrap();

        let error = ensure_world_contract(
            world.path(),
            mc_world::OVERWORLD_GEOMETRY,
            0,
            "vanilla_like",
            "geological_deposits",
            "vanilla",
        )
        .unwrap_err();

        assert!(error.to_string().contains("unversioned Anvil import"));
        assert!(!world_contract_path(world.path()).exists());

        let settlement_error = ensure_world_contract(
            world.path(),
            mc_world::OVERWORLD_GEOMETRY,
            0,
            "vanilla_like",
            "vanilla",
            "plains_village_prototype",
        )
        .unwrap_err();
        assert!(
            settlement_error
                .to_string()
                .contains("unversioned Anvil import")
        );
        assert!(!world_contract_path(world.path()).exists());
    }

    #[test]
    fn world_contract_rejects_seed_and_mode_changes() {
        let world = tempfile::tempdir().unwrap();
        let geometry = mc_world::OVERWORLD_GEOMETRY;

        assert_eq!(
            ensure_world_contract(
                world.path(),
                geometry,
                11,
                "vanilla_like",
                "vanilla",
                "vanilla",
            )
            .unwrap(),
            WorldSource::SolarisGenerated,
        );
        assert_eq!(
            ensure_world_contract(
                world.path(),
                geometry,
                11,
                "vanilla_like",
                "vanilla",
                "vanilla",
            )
            .unwrap(),
            WorldSource::SolarisGenerated,
        );

        let seed_error = ensure_world_contract(
            world.path(),
            geometry,
            12,
            "vanilla_like",
            "vanilla",
            "vanilla",
        )
        .unwrap_err();
        assert!(seed_error.to_string().contains("seed=11"));
        assert!(seed_error.to_string().contains("seed=12"));

        let mode_error = ensure_world_contract(
            world.path(),
            geometry,
            11,
            "tellus_like",
            "vanilla",
            "vanilla",
        )
        .unwrap_err();
        assert!(mode_error.to_string().contains("mode=vanilla_like"));
        assert!(mode_error.to_string().contains("mode=tellus_like"));

        let profile_error = ensure_world_contract(
            world.path(),
            geometry,
            11,
            "vanilla_like",
            "geological_deposits",
            "vanilla",
        )
        .unwrap_err();
        assert!(profile_error.to_string().contains("ore_profile=vanilla"));
        assert!(
            profile_error
                .to_string()
                .contains("ore_profile=geological_deposits")
        );

        let settlement_error = ensure_world_contract(
            world.path(),
            geometry,
            11,
            "vanilla_like",
            "vanilla",
            "plains_village_prototype",
        )
        .unwrap_err();
        assert!(
            settlement_error
                .to_string()
                .contains("settlement_profile=vanilla")
        );
        assert!(
            settlement_error
                .to_string()
                .contains("settlement_profile=plains_village_prototype")
        );

        assert!(world_contract_path(world.path()).is_file());
    }

    #[test]
    fn startup_chunk_workers_cover_configured_and_available_parallelism() {
        let available = std::thread::available_parallelism()
            .map(std::num::NonZeroUsize::get)
            .unwrap_or(1);

        assert_eq!(startup_chunk_worker_threads(0), available.max(1));
        assert_eq!(startup_chunk_worker_threads(available + 3), available + 3);
        assert_eq!(startup_light_bake_worker_threads(0), 1);
        assert_eq!(
            startup_light_bake_worker_threads(available + 3),
            ((available + 3) * 2).min(STARTUP_LIGHT_BAKE_WORKER_CAP)
        );
        assert_eq!(
            startup_light_bake_worker_threads(100),
            STARTUP_LIGHT_BAKE_WORKER_CAP
        );
    }

    #[test]
    fn build_terrain_generator_rejects_missing_required_block() {
        let blocks = terrain_registry_missing_grass_block();

        let err = match build_terrain_generator(
            42,
            mc_worldgen::WorldgenMode::VanillaLike,
            mc_world::OVERWORLD_GEOMETRY,
            blocks,
            mc_worldgen::StructureRules::none(),
            None,
        ) {
            Ok(_) => panic!("missing required terrain block must fail"),
            Err(err) => err,
        };

        assert!(
            err.to_string().contains("building terrain generator"),
            "{err:#}"
        );
        assert!(
            format!("{err:#}")
                .contains("block registry missing required terrain block minecraft:grass_block"),
            "{err:#}"
        );
    }

    #[test]
    fn build_terrain_generator_propagates_chunk_geometry() {
        let blocks =
            Arc::new(
                mc_world::BlockRegistry::from_report(
                    &mc_data::blocks::solaris_required_blocks_report(),
                )
                .unwrap(),
            );
        let geometry = mc_world::ChunkGeometry::new(0, 256).unwrap();
        let generator = build_terrain_generator(
            42,
            mc_worldgen::WorldgenMode::VanillaLike,
            geometry,
            blocks,
            mc_worldgen::StructureRules::none(),
            None,
        )
        .unwrap();

        let chunk = mc_world::ChunkGenerator::generate(
            generator.as_ref(),
            mc_world::ChunkPos { x: 0, z: 0 },
        );

        assert_eq!(chunk.geometry(), geometry);
        assert_eq!(chunk.sections.len(), 16);
    }

    #[test]
    fn build_terrain_generator_applies_the_prepared_plugin_ore_profile() {
        let blocks =
            Arc::new(
                mc_world::BlockRegistry::from_report(
                    &mc_data::blocks::solaris_required_blocks_report(),
                )
                .unwrap(),
            );
        let generator = build_terrain_generator(
            42,
            mc_worldgen::WorldgenMode::VanillaLike,
            mc_world::OVERWORLD_GEOMETRY,
            blocks,
            mc_worldgen::StructureRules::none(),
            Some(mc_script::LuaWorldgenOreProfile::GeologicalDeposits),
        )
        .unwrap();

        assert_eq!(generator.ore_generation_profile(), "geological_deposits");
    }

    #[test]
    fn playable_ruin_rules_require_seed_zero_vanilla_like_profile() {
        let blocks =
            Arc::new(
                mc_world::BlockRegistry::from_report(
                    &mc_data::blocks::solaris_required_blocks_report(),
                )
                .unwrap(),
            );
        let items = mc_data::items::solaris_required_items();

        let playable = structure_rules_for_startup(
            0,
            mc_server::WorldgenMode::VanillaLike,
            None,
            &blocks,
            &items,
            None,
        )
        .unwrap();
        let unrelated_seed = structure_rules_for_startup(
            7,
            mc_server::WorldgenMode::VanillaLike,
            None,
            &blocks,
            &items,
            None,
        )
        .unwrap();
        let unrelated_mode = structure_rules_for_startup(
            0,
            mc_server::WorldgenMode::TellusLike,
            None,
            &blocks,
            &items,
            None,
        )
        .unwrap();

        assert!(!playable.is_empty());
        assert!(unrelated_seed.is_empty());
        assert!(unrelated_mode.is_empty());
    }

    #[test]
    fn settlement_profile_requires_the_vanilla_sidecar() {
        let blocks = mc_world::BlockRegistry::from_report(
            &mc_data::blocks::solaris_required_blocks_report(),
        )
        .unwrap();
        let items = mc_data::items::solaris_required_items();

        let error = structure_rules_for_startup(
            0,
            mc_server::WorldgenMode::TellusLike,
            None,
            &blocks,
            &items,
            Some(&mc_script::LuaSettlementPlan::plains_village_prototype(
                "test-settlement",
            )),
        )
        .unwrap_err();

        assert!(error.to_string().contains("requires data.vanilla_data_dir"));
    }

    #[test]
    fn settlement_profile_loads_the_extracted_prototype_when_present() {
        let vanilla_data_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
        let fountain = vanilla_data_dir
            .join("data/minecraft/structure/village/plains/town_centers/plains_fountain_01.nbt");
        if !fountain.exists() {
            return;
        }
        let blocks = mc_world::BlockRegistry::from_report(
            &mc_data::blocks::solaris_required_blocks_report(),
        )
        .unwrap();
        let items = mc_data::items::solaris_required_items();

        let rules = structure_rules_for_startup(
            0,
            mc_server::WorldgenMode::TellusLike,
            Some(&vanilla_data_dir),
            &blocks,
            &items,
            Some(&mc_script::LuaSettlementPlan::plains_village_prototype(
                "test-settlement",
            )),
        )
        .unwrap();

        assert_eq!(rules.templates().len(), 1);
        assert!(rules.templates()[0].blocks().len() > 200);
    }

    #[test]
    fn extracted_village_prototype_generates_deterministically_when_present() {
        let vanilla_data_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
        let fountain = vanilla_data_dir
            .join("data/minecraft/structure/village/plains/town_centers/plains_fountain_01.nbt");
        if !fountain.exists() {
            return;
        }
        let blocks =
            Arc::new(
                mc_world::BlockRegistry::from_report(
                    &mc_data::blocks::solaris_required_blocks_report(),
                )
                .unwrap(),
            );
        let items = mc_data::items::solaris_required_items();
        let rules = structure_rules_for_startup(
            0,
            mc_server::WorldgenMode::TellusLike,
            Some(&vanilla_data_dir),
            &blocks,
            &items,
            Some(&mc_script::LuaSettlementPlan::plains_village_prototype(
                "test-settlement",
            )),
        )
        .unwrap();
        let first = build_terrain_generator(
            0,
            mc_server::WorldgenMode::TellusLike.to_worldgen(),
            mc_world::OVERWORLD_GEOMETRY,
            Arc::clone(&blocks),
            rules.clone(),
            None,
        )
        .unwrap();
        let second = build_terrain_generator(
            0,
            mc_server::WorldgenMode::TellusLike.to_worldgen(),
            mc_world::OVERWORLD_GEOMETRY,
            Arc::clone(&blocks),
            rules,
            None,
        )
        .unwrap();
        let baseline = build_terrain_generator(
            0,
            mc_server::WorldgenMode::TellusLike.to_worldgen(),
            mc_world::OVERWORLD_GEOMETRY,
            blocks,
            mc_worldgen::StructureRules::none(),
            None,
        )
        .unwrap();
        let mut changed = 0;
        for chunk_x in 3..=5 {
            for chunk_z in -1..=1 {
                let pos = mc_world::ChunkPos {
                    x: chunk_x,
                    z: chunk_z,
                };
                let first_chunk = mc_world::ChunkGenerator::generate(first.as_ref(), pos);
                let second_chunk = mc_world::ChunkGenerator::generate(second.as_ref(), pos);
                let baseline_chunk = mc_world::ChunkGenerator::generate(baseline.as_ref(), pos);
                for y in mc_world::OVERWORLD_GEOMETRY.min_y()..mc_world::OVERWORLD_GEOMETRY.max_y()
                {
                    for local_z in 0..16 {
                        for local_x in 0..16 {
                            let generated = first_chunk.get_block(local_x, y, local_z);
                            assert_eq!(
                                generated,
                                second_chunk.get_block(local_x, y, local_z),
                                "same profile and seed must reproduce every village block"
                            );
                            changed += usize::from(
                                generated != baseline_chunk.get_block(local_x, y, local_z),
                            );
                        }
                    }
                }
            }
        }
        assert!(changed > 200, "prototype changed only {changed} blocks");
    }

    #[test]
    fn effective_protocol_data_rejects_missing_sidecar_root() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("missing-vanilla");

        let err = match load_effective_protocol_data(Some(&missing)) {
            Ok(_) => panic!("missing sidecar root must fail"),
            Err(err) => err,
        };

        assert!(
            err.to_string()
                .contains("reading vanilla sidecar directory metadata")
        );
    }

    #[test]
    fn effective_protocol_data_rejects_mismatched_sidecar_version() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("version.json"),
            format!(
                r#"{{"id":"{}","world_version":{},"protocol_version":999999}}"#,
                mc_protocol::TARGET_RELEASE,
                mc_protocol::WORLD_VERSION,
            ),
        )
        .unwrap();

        let err = match load_effective_protocol_data(Some(tmp.path())) {
            Ok(_) => panic!("mismatched sidecar version must fail before registry loading"),
            Err(err) => err,
        };

        assert!(
            format!("{err:#}").contains("protocol_version 999999 does not match"),
            "{err:#}"
        );
    }

    #[test]
    fn effective_tags_reject_empty_vanilla_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let reports = tmp.path().join("reports");
        std::fs::create_dir_all(&reports).unwrap();
        std::fs::write(reports.join("registries.json"), "{}").unwrap();
        let data = mc_data::VanillaData::from_registries("", vec![]);
        let items = mc_data::items::ItemRegistry::default();

        let err = match load_effective_tags(Some(tmp.path()), &data, &items, &[]) {
            Ok(_) => panic!("empty vanilla tag sidecar must fail"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("vanilla tags"));
        assert!(err.to_string().contains("were empty"));
    }

    #[test]
    fn effective_tags_attach_fuel_values_to_embedded_startup_data() {
        let items = mc_data::items::solaris_required_items();
        let blocks = mc_data::blocks::solaris_required_blocks_report();

        let effective =
            load_effective_tags(None, &mc_data::solaris_required_data(), &items, &blocks).unwrap();
        let oak_stairs = items
            .id_of(&Identifier::parse("minecraft:oak_stairs").unwrap())
            .unwrap();
        let warped_stairs = items
            .id_of(&Identifier::parse("minecraft:warped_stairs").unwrap())
            .unwrap();

        assert_eq!(
            effective.tags.fuel_values().burn_duration(oak_stairs),
            Some(300)
        );
        assert!(!effective.tags.fuel_values().is_fuel(warped_stairs));
    }

    #[test]
    fn effective_tags_reject_partial_fuel_membership_with_all_required_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let reports = tmp.path().join("reports");
        std::fs::create_dir_all(&reports).unwrap();
        std::fs::write(
            reports.join("registries.json"),
            r#"{
                "minecraft:block":{"entries":{"minecraft:stone":{"protocol_id":0}}},
                "minecraft:item":{"entries":{"minecraft:coal":{"protocol_id":10}}},
                "minecraft:entity_type":{"entries":{"minecraft:pig":{"protocol_id":1}}}
            }"#,
        )
        .unwrap();
        for (registry, entry) in [
            ("block", "minecraft:stone"),
            ("entity_type", "minecraft:pig"),
        ] {
            let root = tmp.path().join("data/minecraft/tags").join(registry);
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(
                root.join("sample.json"),
                format!(r#"{{"values":["{entry}"]}}"#),
            )
            .unwrap();
        }
        let item_tags = tmp.path().join("data/minecraft/tags/item");
        std::fs::create_dir_all(&item_tags).unwrap();
        for tag in [
            "logs",
            "bamboo_blocks",
            "planks",
            "wooden_stairs",
            "wooden_slabs",
            "wooden_trapdoors",
            "wooden_pressure_plates",
            "wooden_shelves",
            "wooden_fences",
            "fence_gates",
            "banners",
            "signs",
            "hanging_signs",
            "wooden_doors",
            "boats",
            "wool",
            "wooden_buttons",
            "saplings",
            "wool_carpets",
            "non_flammable_wood",
        ] {
            let values = if tag == "logs" {
                r#"["minecraft:coal"]"#
            } else {
                "[]"
            };
            std::fs::write(
                item_tags.join(format!("{tag}.json")),
                format!(r#"{{"values":{values}}}"#),
            )
            .unwrap();
        }
        let items = mc_data::items::ItemRegistry::from_report(&[mc_data::items::ItemReport {
            id: Identifier::parse("minecraft:coal").unwrap(),
            protocol_id: 10,
        }]);

        let err = match load_effective_tags(
            Some(tmp.path()),
            &mc_data::VanillaData::from_registries("", vec![]),
            &items,
            &[],
        ) {
            Ok(_) => panic!("partial canonical fuel membership must fail startup"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("canonical 26.1.2 default set"));
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

        let err = match load_effective_tags(Some(tmp.path()), &data, &items, &[]) {
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

        let err = match load_effective_tags(Some(tmp.path()), &data, &items, &[]) {
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
    fn effective_item_facts_reject_missing_sidecar_report() {
        let tmp = tempfile::tempdir().unwrap();

        let error = match load_effective_item_facts(Some(tmp.path())) {
            Ok(_) => panic!("missing item component report must fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("were empty"));
        assert!(error.to_string().contains("components/item"));
    }

    #[test]
    fn effective_block_mining_requires_sidecar_file_when_configured() {
        let tmp = tempfile::tempdir().unwrap();
        let report = [mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 0,
                default: true,
                properties: BTreeMap::new(),
            }],
        }];

        let error = match load_effective_block_mining(Some(tmp.path()), &report) {
            Ok(_) => panic!("missing block_mining.json must fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("block-mining table"));
        assert!(error.to_string().contains("block_mining.json"));
    }

    #[test]
    fn effective_block_mining_loads_matching_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let reports = tmp.path().join("reports");
        std::fs::create_dir_all(&reports).unwrap();
        std::fs::write(
            reports.join("block_mining.json"),
            format!(
                r#"{{"version":"{}","max_state_id":1,"entries":[[0.0,0],[1.5,1]]}}"#,
                mc_protocol::TARGET_RELEASE
            ),
        )
        .unwrap();
        let report = [mc_data::blocks::BlockReport {
            id: Identifier::parse("minecraft:stone").unwrap(),
            properties: BTreeMap::new(),
            states: vec![mc_data::blocks::BlockStateReport {
                id: 1,
                default: true,
                properties: BTreeMap::new(),
            }],
        }];

        let effective = load_effective_block_mining(Some(tmp.path()), &report).unwrap();

        assert_eq!(effective.source, "vanilla_sidecar");
        assert_eq!(
            effective.table.as_ref().and_then(|table| table.facts(1)),
            Some(mc_data::block_mining::BlockMiningFacts {
                destroy_speed: 1.5,
                requires_correct_tool_for_drops: true,
            })
        );
    }

    #[test]
    fn effective_item_facts_load_sidecar_tool_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let items = tmp
            .path()
            .join("reports")
            .join("minecraft")
            .join("components")
            .join("item");
        std::fs::create_dir_all(&items).unwrap();
        std::fs::write(
            items.join("wooden_pickaxe.json"),
            r##"{
                "components": {
                    "minecraft:tool": {
                        "rules": [{
                            "blocks": "#minecraft:mineable/pickaxe",
                            "speed": 2.0,
                            "correct_for_drops": true
                        }]
                    }
                }
            }"##,
        )
        .unwrap();

        let effective = load_effective_item_facts(Some(tmp.path())).unwrap();

        assert_eq!(effective.source, "vanilla_sidecar");
        let tool = effective
            .table
            .get(&Identifier::parse("minecraft:wooden_pickaxe").unwrap())
            .and_then(|facts| facts.tool.as_ref())
            .expect("tool facts");
        assert_eq!(tool.rules.len(), 1);
        assert_eq!(tool.rules[0].speed, Some(2.0));
    }

    #[test]
    fn effective_item_facts_use_embedded_fallback_without_sidecar() {
        let effective = load_effective_item_facts(None).unwrap();

        assert_eq!(effective.source, "embedded_solaris_fallback");
        assert!(!effective.table.is_empty());
    }

    #[test]
    fn effective_block_light_rejects_sidecar_that_does_not_cover_blocks_report() {
        let tmp = tempfile::tempdir().unwrap();
        let reports = tmp.path().join("reports");
        std::fs::create_dir_all(&reports).unwrap();
        std::fs::write(
            reports.join("block_light.json"),
            r#"{"version":"26.1.2-test","max_state_id":0,"entries":[[0,0,1,0]]}"#,
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
            r#"{"version":"not-the-target","max_state_id":0,"entries":[[0,0,1,0]]}"#,
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
                  "name": "minecraft:diamond"
                }]
              }]
            }"#,
        )
        .unwrap();

        let loot = load_effective_loot(Some(tmp.path())).unwrap();

        assert_eq!(
            loot.tables.total_drops(),
            mc_data::loot::builtin().total_drops()
        );
        assert_eq!(
            loot.tables
                .block_drop(&Identifier::parse("minecraft:stone").unwrap()),
            Some(&Identifier::parse("minecraft:diamond").unwrap())
        );
        assert_eq!(
            loot.tables
                .entity_drop_stacks(&Identifier::parse("minecraft:cow").unwrap())
                .map(|drops| drops.iter().map(|drop| &drop.item).collect::<Vec<_>>()),
            Some(vec![
                &Identifier::parse("minecraft:leather").unwrap(),
                &Identifier::parse("minecraft:beef").unwrap(),
            ])
        );
        assert_eq!(
            loot.source,
            "vanilla_sidecar_simple_subset+embedded_fallback"
        );
    }

    #[test]
    fn effective_loot_completes_partial_entity_table_from_fallback() {
        let tmp = tempfile::tempdir().unwrap();
        let entities = tmp
            .path()
            .join("data")
            .join("minecraft")
            .join("loot_table")
            .join("entities");
        std::fs::create_dir_all(&entities).unwrap();
        std::fs::write(
            entities.join("sheep.json"),
            r#"{
              "pools": [
                {
                  "entries": [{
                    "type": "minecraft:item",
                    "functions": [{
                      "function": "minecraft:set_count",
                      "count": {
                        "type": "minecraft:uniform",
                        "min": 1.0,
                        "max": 2.0
                      }
                    }],
                    "name": "minecraft:mutton"
                  }]
                },
                {
                  "entries": [{
                    "type": "minecraft:loot_table",
                    "value": "minecraft:entities/sheep/white"
                  }]
                }
              ]
            }"#,
        )
        .unwrap();

        let loot = load_effective_loot(Some(tmp.path())).unwrap();

        assert_eq!(
            loot.tables
                .entity_drop_stacks(&Identifier::parse("minecraft:sheep").unwrap())
                .map(|drops| drops.iter().map(|drop| &drop.item).collect::<Vec<_>>()),
            Some(vec![
                &Identifier::parse("minecraft:mutton").unwrap(),
                &Identifier::parse("minecraft:white_wool").unwrap(),
            ])
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

    #[test]
    fn effective_recipes_keep_embedded_display_ids_when_sidecar_is_present() {
        let tmp = tempfile::tempdir().unwrap();
        let recipes = tmp.path().join("data").join("minecraft").join("recipe");
        std::fs::create_dir_all(&recipes).unwrap();
        std::fs::write(
            recipes.join("oak_planks.json"),
            r#"{
              "type": "minecraft:crafting_shapeless",
              "category": "building",
              "ingredients": [{ "tag": "minecraft:oak_logs" }],
              "result": {
                "id": "minecraft:oak_planks",
                "count": 5
              }
            }"#,
        )
        .unwrap();
        std::fs::write(
            recipes.join("zz_sidecar_only.json"),
            r#"{
              "type": "minecraft:crafting_shapeless",
              "category": "misc",
              "ingredients": [{ "item": "minecraft:stick" }],
              "result": {
                "id": "minecraft:stick",
                "count": 1
              }
            }"#,
        )
        .unwrap();

        let recipes = load_effective_recipes(Some(tmp.path())).unwrap();
        let embedded = mc_data::recipes::solaris_required_recipes();
        let oak_planks_index = embedded
            .iter()
            .position(|recipe| recipe.id.as_str() == "minecraft:oak_planks")
            .unwrap();

        assert_eq!(recipes.source, "vanilla_sidecar+stable_embedded_prefix");
        assert_eq!(recipes.recipes.len(), embedded.len() + 1);
        assert_eq!(
            recipes.recipes[..embedded.len()]
                .iter()
                .map(|recipe| &recipe.id)
                .collect::<Vec<_>>(),
            embedded.iter().map(|recipe| &recipe.id).collect::<Vec<_>>()
        );
        assert_eq!(recipes.recipes[oak_planks_index].result.count, 5);
        assert_eq!(
            recipes.recipes.last().unwrap().id.as_str(),
            "minecraft:zz_sidecar_only"
        );
    }

    #[test]
    fn effective_recipes_reject_configured_sidecar_without_supported_recipes() {
        let tmp = tempfile::tempdir().unwrap();

        let err = match load_effective_recipes(Some(tmp.path())) {
            Ok(_) => panic!("configured vanilla recipe sidecar without recipes must fail"),
            Err(err) => err,
        };

        assert!(err.to_string().contains("vanilla recipes"));
        assert!(err.to_string().contains("no supported recipes"));
    }

    #[test]
    fn effective_recipes_use_embedded_fallback_without_sidecar() {
        let recipes = load_effective_recipes(None).unwrap();

        assert_eq!(recipes.source, "embedded_solaris_fallback");
        assert!(
            recipes
                .recipes
                .iter()
                .any(|recipe| recipe.id.as_str() == "minecraft:oak_planks")
        );
    }

    #[tokio::test]
    async fn production_bound_server_performs_final_save_after_internal_shutdown() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(mc_world::BlockRegistry::from_report(&[]).unwrap());
        let items = Arc::new(mc_data::items::ItemRegistry::default());
        let world = Arc::new(tokio::sync::Mutex::new(
            mc_world::WorldStorage::open(tmp.path(), Arc::clone(&blocks))
                .unwrap()
                .with_item_registry(Arc::clone(&items)),
        ));
        let shutdown = mc_net::ShutdownHandle::default();
        let config = mc_net::ServerConfig {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            motd: "shutdown phase test".into(),
            max_players: 0,
            view_distance: 0,
            data: Arc::new(mc_data::testing::stub()),
            blocks,
            world: Some(world),
            tags: Arc::new(mc_data::tags::TagsData::default()),
            recipes: Arc::new(Vec::new()),
            loot: Arc::new(mc_data::loot::LootTables::default()),
            block_light: None,
            items,
            item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
            block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
            entity_types: Arc::new(mc_data::entity_types::solaris_required_entity_types()),
            biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            chunk_pipeline: mc_net::ChunkPipelinePolicy::default(),
            random_tick: mc_net::RandomTickPolicy::default(),
            command_permissions: mc_net::CommandPermissionConfig::new(Vec::<String>::new(), false),
            loader_manifest: None,
            shutdown: shutdown.clone(),
        };
        let bound = mc_net::bind(config).await.expect("bind");
        let metadata = tmp.path().join("solaris").join("world.dat");
        shutdown.request();
        run_bound_server(bound, shutdown)
            .await
            .expect("production entrypoint drains and performs its sole final save");
        assert!(metadata.exists());
    }

    #[test]
    fn generate_spawn_window_materializes_view_square_plus_light_border() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct StubGen {
            active: Arc<AtomicUsize>,
            max_active: Arc<AtomicUsize>,
            first_workers_ready: Arc<std::sync::Barrier>,
            calls: AtomicUsize,
        }

        impl mc_world::ChunkGenerator for StubGen {
            fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
                let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_active.fetch_max(active, Ordering::SeqCst);
                if self.calls.fetch_add(1, Ordering::SeqCst) < 4 {
                    self.first_workers_ready.wait();
                }
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
        let mut storage = mc_world::WorldStorage::in_memory_with_capacity(registry, 32);
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let generator = Arc::new(StubGen {
            active,
            max_active: Arc::clone(&max_active),
            first_workers_ready: Arc::new(std::sync::Barrier::new(4)),
            calls: AtomicUsize::new(0),
        });

        assert_eq!(
            generate_spawn_window(&mut storage, generator, 1, 4, 4, None).unwrap(),
            25
        );
        assert_eq!(storage.cache_len(), 25);
        assert_eq!(storage.dirty_count(), 25);
        assert!(
            max_active.load(Ordering::SeqCst) > 1,
            "startup pre-generation should use worker threads"
        );
    }

    #[test]
    fn generate_spawn_window_rejects_worker_panic() {
        struct PanicGen;

        impl mc_world::ChunkGenerator for PanicGen {
            fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
                assert_ne!(pos, mc_world::ChunkPos { x: 0, z: 0 }, "worker failure");
                let air = mc_world::BlockStateId(0);
                let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
                mc_world::Chunk::empty(pos, air, biome)
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
        let mut storage = mc_world::WorldStorage::in_memory_with_capacity(registry, 32);

        let error = generate_spawn_window(&mut storage, Arc::new(PanicGen), 1, 4, 4, None)
            .expect_err("partial generation must fail startup");

        assert!(
            error
                .to_string()
                .contains("spawn pre-generation worker panicked"),
            "{error:#}"
        );
        assert!(storage.cache_len() < 25);
    }

    #[test]
    fn generate_spawn_window_bakes_view_square_light() {
        struct StubGen;

        impl mc_world::ChunkGenerator for StubGen {
            fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
                let air = mc_world::BlockStateId(0);
                let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
                let mut chunk = mc_world::Chunk::empty(pos, air, biome);
                chunk.status = "minecraft:full".into();
                chunk.mark_dirty();
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
        let mut storage = mc_world::WorldStorage::in_memory_with_capacity(registry, 32);
        let table = mc_data::block_light::BlockLightTable::from_arrays(
            "test",
            vec![0],
            vec![0],
            vec![true],
        );

        assert_eq!(
            generate_spawn_window(&mut storage, Arc::new(StubGen), 1, 4, 8, Some(&table)).unwrap(),
            25
        );

        for z in -1..=1 {
            for x in -1..=1 {
                let chunk = storage
                    .cached_chunk_snapshot(mc_world::ChunkPos { x, z })
                    .expect("view-square chunk should be resident");
                assert!(
                    mc_world::light::ChunkLight::from_section_lights(&chunk.section_lights)
                        .is_some(),
                    "view-square chunk ({x}, {z}) should carry baked startup light"
                );
            }
        }
    }

    #[test]
    fn warm_spawn_window_loads_existing_chunks_without_dirtying_them() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
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
        {
            let mut storage =
                mc_world::WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 32)
                    .unwrap();
            for pos in spawn_window_positions(1) {
                let mut chunk = mc_world::Chunk::empty(
                    pos,
                    mc_world::BlockStateId(0),
                    mc_data::Identifier::parse("minecraft:plains").unwrap(),
                );
                chunk.mark_dirty();
                storage.insert_generated_chunk(pos, chunk).unwrap();
            }
            assert_eq!(storage.flush_dirty().unwrap(), 25);
        }
        let mut reopened =
            mc_world::WorldStorage::open_with_capacity(tmp.path(), registry, 32).unwrap();

        assert_eq!(warm_spawn_window(&mut reopened, 1).unwrap(), 25);

        assert_eq!(reopened.cache_len(), 25);
        assert_eq!(reopened.dirty_count(), 0);
    }

    #[test]
    fn existing_world_startup_bakes_missing_view_square_light() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
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
        {
            let mut storage =
                mc_world::WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 32)
                    .unwrap();
            for pos in spawn_window_positions(1) {
                let mut chunk = mc_world::Chunk::empty(
                    pos,
                    mc_world::BlockStateId(0),
                    mc_data::Identifier::parse("minecraft:plains").unwrap(),
                );
                chunk.mark_dirty();
                storage.insert_generated_chunk(pos, chunk).unwrap();
            }
            assert_eq!(storage.flush_dirty().unwrap(), 25);
        }
        let mut reopened =
            mc_world::WorldStorage::open_with_capacity(tmp.path(), registry, 32).unwrap();
        let table = mc_data::block_light::BlockLightTable::from_arrays(
            "test",
            vec![0],
            vec![0],
            vec![true],
        );

        assert_eq!(warm_spawn_window(&mut reopened, 1).unwrap(), 25);
        let read_view = reopened.read_view();
        assert_eq!(
            bake_missing_spawn_window_light(&mut reopened, &table, 1, 4).unwrap(),
            9
        );

        for pos in spawn_view_positions(1) {
            let chunk = reopened
                .cached_chunk_snapshot(pos)
                .expect("view-square chunk should remain cached");
            assert!(
                mc_world::light::ChunkLight::from_section_lights(&chunk.section_lights).is_some(),
                "view-square chunk ({}, {}) should be backfilled with baked light",
                pos.x,
                pos.z
            );
            let published = read_view
                .snapshot_chunks(&[pos])
                .chunk(pos)
                .expect("view-square chunk should remain published");
            assert!(
                mc_world::light::ChunkLight::from_section_lights(&published.section_lights)
                    .is_some(),
                "view-square chunk ({}, {}) should publish baked light",
                pos.x,
                pos.z
            );
        }
        assert_eq!(reopened.dirty_count(), 9);
    }

    #[test]
    fn existing_world_startup_defers_generated_light_border_flush_when_view_light_is_present() {
        struct StubGen;

        impl mc_world::ChunkGenerator for StubGen {
            fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
                let air = mc_world::BlockStateId(0);
                let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
                let mut chunk = mc_world::Chunk::empty(pos, air, biome);
                chunk.status = "minecraft:full".into();
                chunk
            }
        }

        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
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
        let baked = mc_world::light::ChunkLight::filled(15, 0);
        {
            let mut storage =
                mc_world::WorldStorage::open_with_capacity(tmp.path(), Arc::clone(&registry), 32)
                    .unwrap();
            for pos in spawn_view_positions(1) {
                let mut chunk = mc_world::Chunk::empty(
                    pos,
                    mc_world::BlockStateId(0),
                    mc_data::Identifier::parse("minecraft:plains").unwrap(),
                );
                chunk.set_baked_light(&baked);
                chunk.mark_dirty();
                storage.insert_generated_chunk(pos, chunk).unwrap();
            }
            assert_eq!(storage.flush_dirty().unwrap(), 9);
        }
        let mut reopened = mc_world::WorldStorage::open_with_capacity(tmp.path(), registry, 32)
            .unwrap()
            .with_generator(Arc::new(StubGen));
        let table = mc_data::block_light::BlockLightTable::from_arrays(
            "test",
            vec![0],
            vec![0],
            vec![true],
        );

        let prep = prepare_existing_spawn_window(&mut reopened, &table, 1, 4).unwrap();

        assert_eq!(prep.warmed, 25);
        assert_eq!(prep.baked, 0);
        assert_eq!(
            prep.dirty, 16,
            "generated light-border chunks should remain dirty and resident before listener"
        );
        assert_eq!(
            reopened.dirty_count(),
            16,
            "existing-world startup should defer dirty warm-cache chunks to startup dirty checkpoint"
        );
    }

    #[test]
    fn check_output_marks_autoscale_live_chunk_send_and_normalized_bounds() {
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
        assert_eq!(autoscale["runtime_mode"], "live_adaptive_work_budgets");
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
        assert!(policy.get("worker_pressure_percent").is_none());
    }
}
