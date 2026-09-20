use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, bail};
use mc_data::VanillaData;
use mc_data::biomes::BiomeSpawnRules;
use mc_data::block_facts::BlockFactsTable;
use mc_data::entity_types::EntityTypeRegistry;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_data::loot::LootTables;
use mc_data::recipes::Recipe;
use mc_data::tags::TagsData;
use mc_net::WorldHandle;
use mc_world::BlockRegistry;
use serde::{Deserialize, Serialize};

mod access_control;
mod network;
#[cfg(test)]
use access_control::{MAX_ACCESS_CONTROL_FILE_BYTES, MAX_ACCESS_CONTROL_FILE_ENTRIES};

pub use access_control::{AccessControlLoadReport, OperatorFileOperation, OperatorFileResult};
pub use network::{AutoscaleProfile, AutoscaleSection};
use network::{
    default_allow_local_dev_operators, default_chunk_generate_rate, default_chunk_load_rate,
    default_chunk_prepare_batch_size, default_chunk_result_queue_size, default_chunk_send_rate,
    default_compression_threshold, default_dimension_height, default_dimension_min_y,
    default_friendly_spawn_chunk_budget, default_friendly_spawn_interval_ticks,
    default_hostile_spawn_chunk_budget, default_hostile_spawn_interval_ticks, default_max_players,
    default_random_tick_speed, default_region_cache_size, default_save_interval_ticks,
    default_view_distance,
};

#[cfg(test)]
#[path = "../access_control_file_tests.rs"]
mod access_control_file_tests;
#[cfg(test)]
mod tests;

/// Top-level server configuration loaded from a TOML file at startup.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    pub server: ServerSection,
    pub network: NetworkSection,
    #[serde(default)]
    pub data: DataSection,
    #[serde(default)]
    pub chunk_pipeline: ChunkPipelineSection,
    #[serde(default)]
    pub simulation: SimulationSection,
    #[serde(default)]
    pub admin: AdminSection,
    #[serde(default)]
    pub auth: AuthSection,
    #[serde(default)]
    pub autoscale: AutoscaleSection,
    #[serde(default)]
    pub plugins: PluginSection,
    #[serde(default)]
    pub dashboard: DashboardSection,
    #[serde(default)]
    pub tab_list: TabListSection,
}

/// Identity-level server settings.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSection {
    pub name: String,
    pub motd: String,
    #[serde(default = "default_max_players")]
    pub max_players: u32,
    #[serde(default = "default_view_distance")]
    pub view_distance: i32,
    #[serde(default = "default_view_distance")]
    pub simulation_distance: i32,
}

/// Network-level server settings.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NetworkSection {
    pub bind_address: String,
    pub port: u16,
}

/// Optional first-party operator dashboard settings. Default-off and
/// loopback-bound; exposing it beyond localhost requires an explicit
/// acknowledgement because the dashboard is read-only and unauthenticated.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DashboardSection {
    pub enabled: bool,
    pub bind_address: String,
    pub port: u16,
    pub allow_remote: bool,
}

impl Default for DashboardSection {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_address: "127.0.0.1".to_owned(),
            port: 8080,
            allow_remote: false,
        }
    }
}

impl DashboardSection {
    /// Validate and normalize the dashboard listener endpoint.
    ///
    /// # Errors
    ///
    /// Returns an error when `bind_address` is not a valid IP address or a
    /// non-loopback bind was requested without `allow_remote = true`.
    pub fn validate(&self) -> Result<SocketAddr, String> {
        let ip: IpAddr = self.bind_address.parse().map_err(|_| {
            format!(
                "dashboard.bind_address `{}` is not a valid IP address",
                self.bind_address
            )
        })?;
        if !ip.is_loopback() && !self.allow_remote {
            return Err(format!(
                "dashboard.bind_address {ip} is not loopback; set dashboard.allow_remote = true to deliberately expose the unauthenticated dashboard"
            ));
        }
        Ok(SocketAddr::new(ip, self.port))
    }
}

/// Optional player-list (tab menu) header and footer. Both default to
/// empty, which is the vanilla default (no header/footer lines shown).
/// TOML basic strings support `\n` escapes for multi-line text.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TabListSection {
    pub header: String,
    pub footer: String,
}

/// `world_dir` is the on-disk world save the server reads chunks from
/// at runtime. The library keeps it optional for synthetic network tests,
/// but the `mc-server` binary requires it for both `--check` and `serve`.
/// Built-in settlement profile applied when no plugin deploys a settlement plan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettlementProfile {
    /// Core stock world: Solaris generates the five vanilla village structures
    /// (plains, desert, savanna, snowy, taiga) and their decor from the derived
    /// content cache, so this profile places villages. A deployed component
    /// settlement plan owns settlement content instead and suppresses this lane;
    /// [`Self::PlainsVillagePrototype`] attaches no core villages either.
    #[default]
    Vanilla,
    /// Bounded Solaris prototype, not full vanilla village generation: the
    /// three vanilla plains templates (`plains_fountain_01`,
    /// `plains_small_house_1`, `plains_tool_smith_1`) are combined into one
    /// composite and placed on the vanilla plains village spacing,
    /// separation, and salt read from `vanilla_data_dir`. Needs the sidecar;
    /// it places no desert, savanna, snowy, or taiga villages.
    PlainsVillagePrototype,
}

impl SettlementProfile {
    /// Stable name recorded in the persisted world identity.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Vanilla => "vanilla",
            Self::PlainsVillagePrototype => "plains_village_prototype",
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataSection {
    #[serde(default)]
    pub world_dir: Option<PathBuf>,
    /// Optional vanilla content cache override. Solaris runs on data derived
    /// from the operator's own licensed Minecraft Java installation: when this
    /// is unset the server discovers a derived cache (or imports one from the
    /// local source/Mojang's public metadata before binding), and when it is
    /// set it must name a complete derived cache root. Mojang-owned files stay
    /// outside the repo; nothing is redistributed.
    #[serde(default)]
    pub vanilla_data_dir: Option<PathBuf>,
    /// World seed for the M7 terrain generator. Defaults to `0` —
    /// every run starts on the same terrain unless this is overridden.
    /// Operators bumping this between runs will see fresh terrain in
    /// previously-unflushed chunks; chunks already written to `.mca`
    /// keep their old contents (the on-disk slot wins).
    #[serde(default)]
    pub seed: i64,
    #[serde(default)]
    pub worldgen_mode: WorldgenMode,
    /// Built-in settlement profile used when no deployed plugin supplies a
    /// settlement plan. The default `vanilla` generates the five vanilla
    /// village structures from the derived content cache;
    /// `plains_village_prototype` opts into the interim bounded Solaris
    /// composite (fountain, small house, toolsmith on vanilla plains village
    /// spacing) and attaches no core villages, and needs `vanilla_data_dir`.
    #[serde(default)]
    pub settlement_profile: SettlementProfile,
    /// Lowest generated world Y, inclusive.
    #[serde(default = "default_dimension_min_y")]
    pub min_y: i32,
    /// Generated world height in blocks.
    #[serde(default = "default_dimension_height")]
    pub height: i32,
}

impl Default for DataSection {
    fn default() -> Self {
        Self {
            world_dir: None,
            vanilla_data_dir: None,
            seed: 0,
            worldgen_mode: WorldgenMode::default(),
            settlement_profile: SettlementProfile::default(),
            min_y: default_dimension_min_y(),
            height: default_dimension_height(),
        }
    }
}

impl DataSection {
    /// Validate the configured vertical range against the chunk format.
    ///
    /// # Errors
    ///
    /// Returns an error when the range is not section-aligned, is empty,
    /// overflows, or cannot be represented by the current heightmap format.
    pub fn chunk_geometry(&self) -> Result<mc_world::ChunkGeometry, String> {
        mc_world::ChunkGeometry::new(self.min_y, self.height).ok_or_else(|| {
            format!(
                "data.min_y ({}) and data.height ({}) must define a positive, 16-block-aligned range supported by the chunk heightmap format",
                self.min_y, self.height
            )
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorldgenMode {
    VanillaLike,
    #[default]
    TellusLike,
}

impl WorldgenMode {
    #[must_use]
    pub fn to_worldgen(self) -> mc_worldgen::WorldgenMode {
        match self {
            Self::VanillaLike => mc_worldgen::WorldgenMode::VanillaLike,
            Self::TellusLike => {
                mc_worldgen::WorldgenMode::TellusLike(mc_worldgen::TellusWorldgenSettings::default())
            }
        }
    }
}

/// Chunk preparation, worker, and cache policy. M13 moves chunk work out
/// of the Play socket task in stages; these settings are the stable
/// operator-facing surface for that pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChunkPipelineSection {
    #[serde(default = "default_chunk_send_rate")]
    pub chunk_send_rate: u32,
    #[serde(default = "default_chunk_load_rate")]
    pub chunk_load_rate: u32,
    #[serde(default = "default_chunk_generate_rate")]
    pub chunk_generate_rate: u32,
    #[serde(default)]
    pub chunk_prepare_budget_ms: u64,
    #[serde(default = "default_chunk_prepare_batch_size")]
    pub chunk_prepare_batch_size: usize,
    #[serde(default = "default_chunk_result_queue_size")]
    pub chunk_result_queue_size: usize,
    #[serde(default = "default_region_cache_size")]
    pub region_cache_size: usize,
    #[serde(default = "default_compression_threshold")]
    pub compression_threshold: i32,
    #[serde(default)]
    pub compression_level: Option<u32>,
    /// Optional absolute bound on the pipeline's own worker threads.
    ///
    /// `0` (the default) derives capacity from the process CPU limit: the
    /// chunk IO pool takes a quarter of it and the shared chunk/entity CPU pool
    /// half. A positive value is the operator's explicit bound for that shared
    /// CPU pool, and the startup chunk bake uses the same number instead of the
    /// process CPU count, so a small host - or a gate run that starts several
    /// servers side by side - can stop one server from claiming every core.
    /// This is a bound, not a percentage, and it is never applied to
    /// performance profiles that set their own workload knobs.
    #[serde(default)]
    pub worker_threads: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulationSection {
    #[serde(default = "default_random_tick_speed")]
    pub random_tick_speed: u32,
    #[serde(default = "default_save_interval_ticks")]
    pub save_interval_ticks: u64,
    #[serde(default = "default_friendly_spawn_interval_ticks")]
    pub friendly_spawn_interval_ticks: u64,
    #[serde(default = "default_hostile_spawn_interval_ticks")]
    pub hostile_spawn_interval_ticks: u64,
    #[serde(default = "default_friendly_spawn_chunk_budget")]
    pub friendly_spawn_chunk_budget: usize,
    #[serde(default = "default_hostile_spawn_chunk_budget")]
    pub hostile_spawn_chunk_budget: usize,
}
/// Optional component plugins, loaded from a deployed plugin directory.
///
/// A configured directory is always a WebAssembly component deployment of the
/// `solaris:plugin` contract.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSection {
    #[serde(default)]
    pub directory: Option<PathBuf>,
    #[serde(default)]
    pub strict: bool,
    #[serde(default)]
    pub expected: Vec<String>,
    /// Operator grants per plugin id. A capability a package requests must be
    /// granted here when the deployment requires grants; the package is refused
    /// otherwise instead of running with fewer rights than it asked for.
    #[serde(default)]
    pub grants: BTreeMap<String, PluginGrantSection>,
    /// Explicit operator registrations of WebAssembly precommit hooks, one
    /// `[[plugins.hooks]]` entry each. A package's manifest declares the hooks
    /// it can answer; only a registration here turns one on, so a declaration
    /// alone never changes an edit or a damage request.
    #[serde(default)]
    pub hooks: Vec<PluginHookSection>,
}

impl PluginSection {
    /// Refuse registrations without a component deployment to carry them.
    ///
    /// # Errors
    ///
    /// Returns the mismatch when hooks are registered without a deployment
    /// directory that could contain the registered component.
    pub fn validate_hooks(&self) -> Result<(), String> {
        if self.hooks.is_empty() {
            return Ok(());
        }
        if self.directory.is_none() {
            return Err(
                "[plugins.hooks] registers precommit handlers, but plugins.directory names no component deployment their plugin_id could belong to"
                    .to_owned(),
            );
        }
        Ok(())
    }
}

/// What one operator grants one plugin id.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginGrantSection {
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// One explicit operator registration of a WebAssembly precommit hook.
///
/// This is the operator's decision, not the package's: the registration names
/// the deployed package that must answer the hook, where it sits in the chain,
/// and what happens when it cannot answer. Nothing here is derived from a
/// guest's manifest, and a package that never declares the hook cannot be
/// registered for it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginHookSection {
    /// The deployed package id this registration authorizes.
    pub plugin_id: String,
    /// Which precommit hook this registration turns on.
    pub kind: PluginHookKind,
    /// Chain position. Registrations run in ascending `(order, plugin_id)`.
    #[serde(default)]
    pub order: i32,
    /// What the chain does when this handler fails. Denied by default: a hook
    /// that cannot answer must never silently become an allowed edit.
    #[serde(default)]
    pub on_failure: PluginHookFailure,
}

/// Which precommit hook one `[[plugins.hooks]]` registration turns on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginHookKind {
    BeforeBuild,
    BeforeDamage,
}

/// What the chain does when a registered handler fails.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginHookFailure {
    /// Refuse the edit or the damage: the fail-closed default.
    #[default]
    Deny,
    /// Keep the unmodified request.
    Keep,
}

impl PluginHookSection {
    /// The registration the native host's roster is built from.
    ///
    /// The operator's TOML spelling and the contract's native names are not the
    /// same words, and this is the only place they are translated.
    #[must_use]
    pub fn registration(&self) -> mc_script::precommit::HookRegistration {
        mc_script::precommit::HookRegistration::new(
            self.plugin_id.clone(),
            match self.kind {
                PluginHookKind::BeforeBuild => mc_script::precommit::HookKind::Build,
                PluginHookKind::BeforeDamage => mc_script::precommit::HookKind::Damage,
            },
            self.order,
            match self.on_failure {
                PluginHookFailure::Deny => mc_script::precommit::HookFailurePolicy::Deny,
                PluginHookFailure::Keep => mc_script::precommit::HookFailurePolicy::Keep,
            },
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSection {
    #[serde(default)]
    pub operators: Vec<String>,
    #[serde(default)]
    pub operators_file: Option<PathBuf>,
    #[serde(default = "default_allow_local_dev_operators")]
    pub allow_local_dev_operators: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AuthSection {
    #[serde(default)]
    pub online_mode: bool,
    #[serde(default)]
    pub prevent_proxy_connections: bool,
    #[serde(default)]
    pub whitelist_enabled: bool,
    #[serde(default)]
    pub whitelist: Vec<String>,
    #[serde(default)]
    pub whitelist_file: Option<PathBuf>,
    #[serde(default)]
    pub banned_players: Vec<String>,
    #[serde(default)]
    pub banned_players_file: Option<PathBuf>,
}
