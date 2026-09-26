use super::*;
use sha2::{Digest, Sha256};

pub(crate) fn check_config(path: &Path) -> Result<()> {
    let mut cfg = load_config(path)?;
    cfg.load_access_control_files(path)?;
    let _ip: IpAddr = cfg.network.bind_address.parse().with_context(|| {
        format!(
            "validating network.bind_address `{}`",
            cfg.network.bind_address
        )
    })?;
    if cfg.dashboard.enabled {
        cfg.dashboard.validate().map_err(anyhow::Error::msg)?;
    }
    validate_runtime_config(&cfg)?;
    let Some(deployment) = component_deployment(&cfg)? else {
        let effective = EffectiveConfig::from(&cfg);
        let rendered =
            serde_json::to_string_pretty(&effective).context("rendering config as JSON")?;
        println!("{rendered}");
        return Ok(());
    };
    // `--check` of a component deployment compiles every selected component and
    // runs its startup phases against a boundary nobody drains: it reports the
    // contract, artifacts, and startup answers it really exercised while
    // touching no world, storage, or listener.
    let report =
        mc_plugin_host::check_deployment(&deployment, &mc_plugin_host::PluginLimits::default())
            .map_err(|error| anyhow::anyhow!("checking component plugins: {error}"))?;
    // A check validates the whole selection: a package the deployment skipped
    // is a package it could not validate, so the check fails instead of
    // reporting success for a deployment it did not read end to end.
    if let Some(skipped) = report.skipped().first() {
        bail!(
            "checking component plugins: {} was refused ({})",
            skipped.path.display(),
            skipped.message
        );
    }
    mc_net::LoaderManifest::from_script_bundles(report.client_bundles())
        .context("reading Solaris Loader artifact identities")?
        .encode()
        .context("encoding aggregated Solaris Loader manifest")?;
    let ids = report
        .checked()
        .iter()
        .map(|checked| checked.id.clone())
        .collect::<Vec<_>>();
    println!("component plugins checked: {}", ids.join(", "));
    Ok(())
}

#[derive(serde::Serialize)]
pub(crate) struct EffectiveConfig<'a> {
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
pub(crate) struct OperatorWarning {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
}

/// The honest operator notice for the vanilla village terrain analogue.
///
/// Vanilla villages carry `terrain_adaptation = beard_thin`, a 3D density term
/// vanilla adds inside `NoiseChunk`. Solaris terrain is a 2D per-column surface
/// with no density array for that term to enter, so the same Beardifier kernel,
/// weights and `box.minY() + groundLevelDelta` targets are applied as a
/// **column-height analogue**: each column's surface is pulled toward the rigid
/// pieces' ground level. The village geometry is vanilla's; the terrain around
/// it is not, and the operator is told so rather than discovering it in a diff.
pub(crate) const VILLAGE_BEARD_ANALOGUE_NOTICE: OperatorWarning = OperatorWarning {
    code: "village_terrain_adaptation_beard_thin_is_a_column_height_analogue",
    message: "vanilla villages generate with terrain_adaptation = beard_thin applied as a Solaris column-height analogue, not as vanilla's density arithmetic: the terrain around a village is pulled toward the rigid pieces' ground level by the Beardifier kernel (crates/mc-worldgen/src/village/beard.rs), so village surroundings differ from vanilla",
};

/// The notice the operator is owed for a generator that places vanilla
/// villages, `Some` exactly when it does: the analogue is not something a
/// village world can be built without, so it is reported with the build rather
/// than promised in a document.
pub(crate) fn village_terrain_analogue_notice(
    village_analogue: bool,
) -> Option<&'static OperatorWarning> {
    village_analogue.then_some(&VILLAGE_BEARD_ANALOGUE_NOTICE)
}

#[derive(serde::Deserialize)]
pub(crate) struct VanillaVersionMetadata {
    id: String,
    world_version: u32,
    protocol_version: i32,
}

pub(crate) fn operator_warnings(config: &ServerConfig) -> Vec<OperatorWarning> {
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
            message: "offline-mode public server: player names and operator identities are not authenticated",
        });
    }
    warnings
}

pub(crate) fn vanilla_registry_tree_is_complete(vanilla_data_dir: &Path) -> bool {
    let minecraft_root = vanilla_data_dir.join("data").join("minecraft");
    minecraft_root.is_dir()
        && mc_data::KNOWN_REGISTRIES
            .iter()
            .all(|(_, fs_subpath)| registry_dir_has_json(&minecraft_root.join(fs_subpath)))
}

pub(crate) fn registry_dir_has_json(path: &Path) -> bool {
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

pub(crate) fn vanilla_block_light_report_matches_target(vanilla_data_dir: &Path) -> bool {
    let path = vanilla_data_dir.join("reports").join("block_light.json");
    mc_data::block_light::load(path).is_ok_and(|table| table.version == mc_protocol::TARGET_RELEASE)
}

pub(crate) fn vanilla_tags_are_usable(vanilla_data_dir: &Path) -> bool {
    let Ok(protocol_data) = load_effective_protocol_data(vanilla_data_dir) else {
        return false;
    };
    let items = mc_data::items::solaris_required_items();
    load_effective_tags(vanilla_data_dir, &protocol_data, &items).is_ok()
}

pub(crate) fn vanilla_recipes_are_usable(vanilla_data_dir: &Path) -> bool {
    load_effective_recipes(vanilla_data_dir).is_ok()
}

pub(crate) fn vanilla_loot_is_usable(vanilla_data_dir: &Path) -> bool {
    load_effective_loot(vanilla_data_dir).is_ok()
}

#[derive(serde::Serialize)]
pub(crate) struct EffectiveChunkPipeline {
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
pub(crate) struct EffectiveAutoscale {
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
                config
                    .autoscale
                    .to_policy(&config.server, &config.chunk_pipeline),
            ),
        }
    }
}

#[derive(serde::Serialize)]
pub(crate) struct EffectiveAutoscaleLimits {
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
pub(crate) struct EffectiveAutoscalePolicy {
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
    scale_down_after_seconds: u32,
    scale_up_after_seconds: u32,
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
            scale_down_after_seconds: policy.scale_down_after_seconds,
            scale_up_after_seconds: policy.scale_up_after_seconds,
        }
    }
}

/// The world-startup records one component deployment contributes.
///
/// A component host reads these records after `configure` runs and before the
/// server opens its world. The host rejects conflicting declarations, so the
/// records describe the one deployment the server will run.
pub(crate) struct DeploymentRecords<'a> {
    pub(crate) rules: Option<&'a mc_script::GameplayRules>,
    pub(crate) items: &'a [mc_data::item_components::CustomItemDefinition],
    pub(crate) ore_profile: Option<mc_script::PluginWorldgenOreProfile>,
    pub(crate) settlement_plan: Option<&'a mc_script::PluginSettlementPlan>,
    pub(crate) client_bundles: &'a [mc_script::ClientBundle],
}

impl<'a> DeploymentRecords<'a> {
    /// Read the records of the component deployment this server prepared.
    pub(crate) fn of(component: Option<&'a PreparedComponent>) -> Self {
        match component {
            Some(component) => Self {
                rules: component.rules.as_ref(),
                items: &component.items,
                ore_profile: component.ore_profile,
                settlement_plan: component.settlement_plan.as_ref(),
                client_bundles: &component.client_bundles,
            },
            None => Self {
                rules: None,
                items: &[],
                ore_profile: None,
                settlement_plan: None,
                client_bundles: &[],
            },
        }
    }

    /// The identities these records resolve to in the persisted world contract.
    ///
    /// `world.json` persists these strings and compares them on every reopen, so
    /// they are derived in one place: a deployment that declares an ore profile
    /// or a settlement plan owns that declaration, and a deployment that declares
    /// neither records the baseline the world is actually generated with.
    pub(crate) fn world_contract_identities(
        &self,
        configured_settlement: mc_server::SettlementProfile,
    ) -> WorldContractIdentities {
        WorldContractIdentities {
            ore_profile: self
                .ore_profile
                .map(mc_script::PluginWorldgenOreProfile::contract_name)
                .unwrap_or("vanilla")
                .to_owned(),
            settlement_profile: self
                .settlement_plan
                .map(mc_script::PluginSettlementPlan::contract_name)
                .unwrap_or_else(|| configured_settlement.name().to_owned()),
            gameplay_rules: self.rules.map(mc_script::GameplayRules::contract_name),
            custom_items: (!self.items.is_empty()).then(|| {
                let mut entries = self
                    .items
                    .iter()
                    .map(|item| {
                        (
                            item.id.as_str(),
                            item.carrier.as_str(),
                            item.name.as_str(),
                            item.crafting_ingredient
                                .as_ref()
                                .map(mc_data::Identifier::as_str),
                            item.facts.max_stack_size,
                            item.facts.max_damage,
                            item.facts.weapon,
                            item.facts.attack_damage_modifier.map(f32::to_bits),
                            item.facts.attack_speed_modifier.map(f32::to_bits),
                            item.facts.equippable_slot.as_deref(),
                        )
                    })
                    .collect::<Vec<_>>();
                entries.sort_unstable_by_key(|item| item.0);
                let bytes =
                    serde_json::to_vec(&entries).expect("validated item identity serializes");
                format!("component-items:{:x}", Sha256::digest(bytes))
            }),
        }
    }

    /// The manifest the Solaris Loader handshake advertises, or `None` for a
    /// deployment that ships no client bundle - the vanilla-client server.
    ///
    /// Building the manifest is also the check the handshake depends on: every
    /// declared block and view is verified against the artifact the package was
    /// loaded from, and the encoded manifest is the exact payload the network
    /// layer sends. A bundle that cannot be advertised fails startup here instead
    /// of on a client's first login.
    pub(crate) fn loader_manifest(&self) -> Result<Option<Arc<mc_net::LoaderManifest>>> {
        if self.client_bundles.is_empty() {
            return Ok(None);
        }
        let manifest = mc_net::LoaderManifest::from_script_bundles(self.client_bundles)
            .context("reading Solaris Loader artifact identities")?;
        if manifest.is_empty() {
            return Ok(None);
        }
        manifest
            .encode()
            .context("encoding aggregated Solaris Loader manifest")?;
        Ok(Some(Arc::new(manifest)))
    }
}

/// Startup identities recorded by each world contract.
pub(crate) struct WorldContractIdentities {
    pub(crate) ore_profile: String,
    pub(crate) settlement_profile: String,
    pub(crate) gameplay_rules: Option<String>,
    pub(crate) custom_items: Option<String>,
}
