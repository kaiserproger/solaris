//! # mc-server
//!
//! Main server binary that ties the Solaris engine together.
//!
//! Part of the Solaris engine.

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
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

/// Crate version, exposed so other crates and the binary can report it.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

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

/// `world_dir` is the on-disk world save the server reads chunks from
/// at runtime. The library keeps it optional for synthetic network tests,
/// but the `mc-server` binary requires it for both `--check` and `serve`.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DataSection {
    #[serde(default)]
    pub world_dir: Option<PathBuf>,
    /// Optional local vanilla data sidecar root. Mojang-owned files stay outside
    /// the repo; when set, Solaris treats the sidecar as authoritative and
    /// requires supported registries, tags, reports, and simple loot data.
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
    #[default]
    VanillaLike,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SimulationSection {
    #[serde(default = "default_random_tick_speed")]
    pub random_tick_speed: u32,
    #[serde(default = "default_save_interval_ticks")]
    pub save_interval_ticks: u64,
    #[serde(default = "default_spawn_monsters")]
    pub spawn_monsters: bool,
}

/// Optional directory containing server-side Lua plugins.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginSection {
    #[serde(default)]
    pub directory: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdminSection {
    #[serde(default)]
    pub operators: Vec<String>,
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
    pub banned_players: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AutoscaleProfile {
    LowEnd,
    #[default]
    Balanced,
    HighEnd,
}

impl AutoscaleProfile {
    #[must_use]
    pub fn to_network(self) -> mc_net::AutoscaleProfile {
        match self {
            Self::LowEnd => mc_net::AutoscaleProfile::LowEnd,
            Self::Balanced => mc_net::AutoscaleProfile::Balanced,
            Self::HighEnd => mc_net::AutoscaleProfile::HighEnd,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoscaleSection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub profile: AutoscaleProfile,
    #[serde(default)]
    pub min_view_distance: Option<i32>,
    #[serde(default)]
    pub max_view_distance: Option<i32>,
    #[serde(default)]
    pub target_tick_ms: Option<u64>,
    #[serde(default)]
    pub target_first_chunk_ms: Option<u64>,
    #[serde(default)]
    pub scale_down_after_ticks: Option<u32>,
    #[serde(default)]
    pub scale_up_after_ticks: Option<u32>,
}

impl Default for AutoscaleSection {
    fn default() -> Self {
        Self {
            enabled: true,
            profile: AutoscaleProfile::Balanced,
            min_view_distance: None,
            max_view_distance: None,
            target_tick_ms: None,
            target_first_chunk_ms: None,
            scale_down_after_ticks: None,
            scale_up_after_ticks: None,
        }
    }
}

impl AutoscaleSection {
    #[must_use]
    pub fn to_policy(&self, chunk_pipeline: &ChunkPipelineSection) -> mc_net::AutoscalePolicy {
        let mut policy = mc_net::AutoscalePolicy::for_profile(self.profile.to_network());
        if let Some(value) = self.min_view_distance {
            policy.min_view_distance = value;
        }
        if let Some(value) = self.max_view_distance {
            policy.max_view_distance = value;
        }
        if let Some(value) = self.target_tick_ms {
            policy.target_tick_ms = value;
        }
        if let Some(value) = self.target_first_chunk_ms {
            policy.target_first_chunk_ms = value;
        }
        if let Some(value) = self.scale_down_after_ticks {
            policy.scale_down_after_ticks = value;
        }
        if let Some(value) = self.scale_up_after_ticks {
            policy.scale_up_after_ticks = value;
        }

        policy.min_chunk_send_rate = policy
            .min_chunk_send_rate
            .min(chunk_pipeline.chunk_send_rate.max(1));
        policy.max_chunk_send_rate = policy
            .max_chunk_send_rate
            .max(chunk_pipeline.chunk_send_rate.max(1));
        policy.min_chunk_load_rate = policy
            .min_chunk_load_rate
            .min(chunk_pipeline.chunk_load_rate.max(1));
        policy.max_chunk_load_rate = policy
            .max_chunk_load_rate
            .max(chunk_pipeline.chunk_load_rate.max(1));
        policy.min_chunk_generate_rate = policy
            .min_chunk_generate_rate
            .min(chunk_pipeline.chunk_generate_rate.max(1));
        policy.max_chunk_generate_rate = policy
            .max_chunk_generate_rate
            .max(chunk_pipeline.chunk_generate_rate.max(1));
        policy.normalized()
    }

    #[must_use]
    pub fn initial_limits(
        &self,
        server: &ServerSection,
        chunk_pipeline: &ChunkPipelineSection,
    ) -> mc_net::RuntimeControlLimits {
        mc_net::RuntimeControlLimits {
            view_distance: server.view_distance,
            chunk_send_rate: chunk_pipeline.chunk_send_rate.max(1),
            chunk_load_rate: chunk_pipeline.chunk_load_rate.max(1),
            chunk_generate_rate: chunk_pipeline.chunk_generate_rate.max(1),
        }
        .bounded(self.to_policy(chunk_pipeline))
    }
}

impl Default for AdminSection {
    fn default() -> Self {
        Self {
            operators: Vec::new(),
            allow_local_dev_operators: default_allow_local_dev_operators(),
        }
    }
}

impl Default for SimulationSection {
    fn default() -> Self {
        let policy = mc_net::RandomTickPolicy::default();
        Self {
            random_tick_speed: policy.random_tick_speed,
            save_interval_ticks: policy.save_interval_ticks,
            spawn_monsters: policy.spawn_monsters,
        }
    }
}

impl SimulationSection {
    #[must_use]
    pub fn to_network(&self, seed: i64, simulation_distance: i32) -> mc_net::RandomTickPolicy {
        let defaults = mc_net::RandomTickPolicy::default();
        mc_net::RandomTickPolicy {
            simulation_distance: simulation_distance
                .clamp(mc_net::MIN_VIEW_DISTANCE, mc_net::MAX_VIEW_DISTANCE),
            random_tick_speed: self.random_tick_speed,
            chunk_budget: defaults.chunk_budget,
            fluid_tick_budget: defaults.fluid_tick_budget,
            save_interval_ticks: self.save_interval_ticks.max(1),
            spawn_monsters: self.spawn_monsters,
            seed: seed as u64,
        }
    }
}

impl Default for ChunkPipelineSection {
    fn default() -> Self {
        let policy = mc_net::ChunkPipelinePolicy::default();
        Self {
            chunk_send_rate: policy.chunk_send_rate,
            chunk_load_rate: policy.chunk_load_rate,
            chunk_generate_rate: policy.chunk_generate_rate,
            chunk_prepare_budget_ms: policy.chunk_prepare_budget_ms,
            chunk_prepare_batch_size: policy.chunk_prepare_batch_size,
            chunk_result_queue_size: policy.chunk_result_queue_size,
            region_cache_size: policy.region_cache_size,
            compression_threshold: policy.compression_threshold,
            compression_level: policy.compression_level,
        }
    }
}

impl ChunkPipelineSection {
    #[must_use]
    pub fn to_network(&self) -> mc_net::ChunkPipelinePolicy {
        let worker_defaults = mc_net::ChunkPipelinePolicy::default();
        mc_net::ChunkPipelinePolicy {
            chunk_send_rate: self.chunk_send_rate.max(1),
            chunk_load_rate: self.chunk_load_rate.max(1),
            chunk_generate_rate: self.chunk_generate_rate.max(1),
            chunk_prepare_budget_ms: self.chunk_prepare_budget_ms,
            chunk_prepare_batch_size: self.chunk_prepare_batch_size.max(1),
            chunk_io_threads: worker_defaults.chunk_io_threads,
            chunk_worker_threads: worker_defaults.chunk_worker_threads,
            chunk_result_queue_size: self.chunk_result_queue_size.max(1),
            region_cache_size: self.region_cache_size.max(1),
            compression_threshold: self.compression_threshold.max(0),
            compression_level: self.compression_level.map(|level| level.min(9)),
            runtime_control: None,
        }
    }
}

fn default_max_players() -> u32 {
    20
}

fn default_view_distance() -> i32 {
    mc_net::DEFAULT_VIEW_DISTANCE
}

fn default_dimension_min_y() -> i32 {
    mc_world::MIN_Y
}

fn default_dimension_height() -> i32 {
    mc_world::MAX_Y - mc_world::MIN_Y
}

fn default_chunk_send_rate() -> u32 {
    mc_net::ChunkPipelinePolicy::default().chunk_send_rate
}

fn default_chunk_load_rate() -> u32 {
    mc_net::ChunkPipelinePolicy::default().chunk_load_rate
}

fn default_chunk_generate_rate() -> u32 {
    mc_net::ChunkPipelinePolicy::default().chunk_generate_rate
}

fn default_chunk_prepare_batch_size() -> usize {
    mc_net::ChunkPipelinePolicy::default().chunk_prepare_batch_size
}

fn default_chunk_result_queue_size() -> usize {
    mc_net::ChunkPipelinePolicy::default().chunk_result_queue_size
}

fn default_region_cache_size() -> usize {
    mc_net::ChunkPipelinePolicy::default().region_cache_size
}

fn default_compression_threshold() -> i32 {
    mc_net::ChunkPipelinePolicy::default().compression_threshold
}

fn default_random_tick_speed() -> u32 {
    mc_net::RandomTickPolicy::default().random_tick_speed
}

fn default_save_interval_ticks() -> u64 {
    mc_net::RandomTickPolicy::default().save_interval_ticks
}

fn default_spawn_monsters() -> bool {
    mc_net::RandomTickPolicy::default().spawn_monsters
}

fn default_allow_local_dev_operators() -> bool {
    false
}

impl ServerConfig {
    /// Convert a parsed TOML config into the network-layer
    /// [`mc_net::ServerConfig`], using the pre-loaded vanilla data,
    /// block registry, and (optionally) a shared world handle.
    #[allow(clippy::too_many_arguments)]
    pub fn to_network(
        &self,
        data: Arc<VanillaData>,
        blocks: Arc<BlockRegistry>,
        world: Option<WorldHandle>,
        tags: Arc<TagsData>,
        recipes: Arc<Vec<Recipe>>,
        loot: Arc<LootTables>,
        block_light: Option<Arc<mc_data::block_light::BlockLightTable>>,
        items: Arc<ItemRegistry>,
        item_facts: Arc<ItemFactsTable>,
        block_facts: Arc<BlockFactsTable>,
        entity_types: Arc<EntityTypeRegistry>,
        biome_spawns: Arc<BiomeSpawnRules>,
    ) -> anyhow::Result<mc_net::ServerConfig> {
        let geometry = self.data.chunk_geometry().map_err(anyhow::Error::msg)?;
        validate_loaded_chunk_geometry(world.as_ref(), geometry)?;
        let ip: IpAddr = self.network.bind_address.parse().with_context(|| {
            format!(
                "invalid network.bind_address {:?}",
                self.network.bind_address
            )
        })?;
        let mut chunk_pipeline = self.chunk_pipeline.to_network();
        if self.autoscale.enabled {
            chunk_pipeline.runtime_control = Some(mc_net::RuntimeControlConfig {
                policy: self.autoscale.to_policy(&self.chunk_pipeline),
                initial_limits: self
                    .autoscale
                    .initial_limits(&self.server, &self.chunk_pipeline),
            });
        }
        Ok(mc_net::ServerConfig {
            bind_address: SocketAddr::new(ip, self.network.port),
            motd: self.server.motd.clone(),
            max_players: self.server.max_players,
            view_distance: self
                .server
                .view_distance
                .clamp(mc_net::MIN_VIEW_DISTANCE, mc_net::MAX_VIEW_DISTANCE),
            data,
            blocks,
            world,
            tags,
            recipes,
            loot,
            block_light,
            items,
            item_facts,
            block_facts,
            entity_types,
            biome_spawns,
            chunk_pipeline,
            random_tick: self
                .simulation
                .to_network(self.data.seed, self.server.simulation_distance),
            command_permissions: mc_net::CommandPermissionConfig::new(
                self.admin.operators.clone(),
                self.admin.allow_local_dev_operators,
            )
            .with_login_access(
                mc_net::LoginAccessConfig::normalized(
                    self.auth.online_mode,
                    self.auth.whitelist_enabled,
                    self.auth.whitelist.clone(),
                    self.auth.banned_players.clone(),
                )
                .with_prevent_proxy_connections(self.auth.prevent_proxy_connections),
            ),
            shutdown: mc_net::ShutdownHandle::default(),
        })
    }
}

fn validate_loaded_chunk_geometry(
    world: Option<&WorldHandle>,
    configured: mc_world::ChunkGeometry,
) -> anyhow::Result<()> {
    let Some(world) = world else {
        return Ok(());
    };
    let storage = world.try_lock().map_err(|_| {
        anyhow::anyhow!("cannot validate loaded chunk geometry: world storage is busy")
    })?;
    for (position, chunk) in storage.resident_chunk_snapshots() {
        let loaded = chunk.geometry();
        if loaded != configured {
            bail!(
                "loaded chunk ({}, {}) has geometry {}..{}, but data config requires {}..{}",
                position.x,
                position.z,
                loaded.min_y(),
                loaded.max_y(),
                configured.min_y(),
                configured.max_y(),
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_playable_profile_as_loopback_survival_spike() {
        let cfg: ServerConfig =
            toml::from_str(include_str!("../../../playable.toml")).expect("parse playable.toml");

        assert_eq!(cfg.server.name, "solaris-playable");
        assert_eq!(cfg.server.motd, "Solaris playable spike");
        assert_eq!(cfg.server.view_distance, 4);
        assert_eq!(cfg.server.simulation_distance, 4);
        assert_eq!(cfg.network.bind_address, "127.0.0.1");
        assert_eq!(cfg.network.port, 25565);
        assert_eq!(
            cfg.data.world_dir,
            Some(std::path::PathBuf::from(".analysis/test-world-v5"))
        );
        assert_eq!(
            cfg.data.vanilla_data_dir,
            Some(std::path::PathBuf::from("data/vanilla"))
        );
        assert_eq!(cfg.data.seed, 0);
        assert_eq!(cfg.data.worldgen_mode, WorldgenMode::VanillaLike);
        assert!(!cfg.auth.online_mode);
        assert!(!cfg.auth.prevent_proxy_connections);
        assert!(!cfg.auth.whitelist_enabled);
        assert!(cfg.auth.whitelist.is_empty());
        assert!(cfg.auth.banned_players.is_empty());
        assert!(cfg.admin.operators.is_empty());
        assert!(!cfg.admin.allow_local_dev_operators);
        assert_eq!(cfg.simulation.random_tick_speed, 5);
        assert_eq!(cfg.simulation.save_interval_ticks, 1200);
        assert!(cfg.simulation.spawn_monsters);
        assert_eq!(cfg.chunk_pipeline.chunk_send_rate, 8);
        assert_eq!(cfg.chunk_pipeline.chunk_load_rate, 16);
        assert_eq!(cfg.chunk_pipeline.chunk_generate_rate, 16);
        assert_eq!(cfg.chunk_pipeline.chunk_result_queue_size, 64);
        assert_eq!(cfg.chunk_pipeline.region_cache_size, 9);
        assert!(cfg.autoscale.enabled);
        assert_eq!(cfg.autoscale.min_view_distance, Some(4));
        assert_eq!(cfg.autoscale.max_view_distance, Some(4));
        let autoscale_policy = cfg.autoscale.to_policy(&cfg.chunk_pipeline);
        assert_eq!(autoscale_policy.min_view_distance, 4);
        assert_eq!(autoscale_policy.max_view_distance, 4);
    }

    #[test]
    fn parses_example_config_shape() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "0.0.0.0"
            port = 25565
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
        assert_eq!(cfg.server.name, "S");
        assert_eq!(cfg.server.max_players, 20);
        assert_eq!(cfg.server.view_distance, 10);
        assert_eq!(cfg.server.simulation_distance, 10);
        assert_eq!(cfg.network.port, 25565);
        assert_eq!(cfg.chunk_pipeline.chunk_prepare_batch_size, 8);
        assert_eq!(cfg.simulation.random_tick_speed, 3);
        assert_eq!(cfg.simulation.save_interval_ticks, 20);
        assert!(cfg.simulation.spawn_monsters);
        assert!(cfg.data.vanilla_data_dir.is_none());
        assert_eq!(cfg.data.worldgen_mode, WorldgenMode::VanillaLike);
        assert_eq!(
            cfg.data.chunk_geometry().unwrap(),
            mc_world::OVERWORLD_GEOMETRY
        );
        assert!(!cfg.admin.allow_local_dev_operators);
        assert!(!cfg.auth.online_mode);
        assert!(!cfg.auth.prevent_proxy_connections);
        assert!(!cfg.auth.whitelist_enabled);
        assert!(cfg.autoscale.enabled);
        assert_eq!(cfg.autoscale.profile, AutoscaleProfile::Balanced);
    }

    #[test]
    fn parses_ip_bound_online_authentication() {
        let cfg: ServerConfig = toml::from_str(
            r#"
                [server]
                name = "S"
                motd = "M"

                [network]
                bind_address = "127.0.0.1"
                port = 25565

                [auth]
                online_mode = true
                prevent_proxy_connections = true
            "#,
        )
        .expect("parse IP-bound online authentication");

        assert!(cfg.auth.online_mode);
        assert!(cfg.auth.prevent_proxy_connections);
    }

    #[test]
    fn parses_explicit_chunk_geometry_and_rejects_invalid_ranges() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565

            [data]
            min_y = 0
            height = 256
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
        let geometry = cfg.data.chunk_geometry().expect("valid geometry");
        assert_eq!(geometry.min_y(), 0);
        assert_eq!(geometry.height(), 256);

        let invalid = DataSection {
            min_y: 1,
            height: 255,
            ..DataSection::default()
        };
        assert!(invalid.chunk_geometry().is_err());
    }

    #[test]
    fn parses_plugin_directory_without_exposing_runtime_tuning() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565

            [plugins]
            directory = "plugins"
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");

        assert_eq!(cfg.plugins.directory, Some(PathBuf::from("plugins")));
    }

    #[test]
    fn example_config_enables_live_autoscale() {
        let cfg: ServerConfig =
            toml::from_str(include_str!("../../../example.toml")).expect("parse example.toml");

        assert!(cfg.autoscale.enabled);
    }

    #[test]
    fn parses_explicit_auth_and_admin_policy() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565

            [admin]
            operators = ["Notch"]
            allow_local_dev_operators = true

            [auth]
            online_mode = false
            whitelist_enabled = true
            whitelist = ["Notch"]
            banned_players = ["BadActor"]
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
        assert_eq!(cfg.admin.operators, ["Notch"]);
        assert!(cfg.admin.allow_local_dev_operators);
        assert!(cfg.auth.whitelist_enabled);
        assert_eq!(cfg.auth.whitelist, ["Notch"]);
        assert_eq!(cfg.auth.banned_players, ["BadActor"]);
    }

    #[test]
    fn parses_optional_vanilla_data_dir() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "0.0.0.0"
            port = 25565

            [data]
            vanilla_data_dir = "data/vanilla"
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
        assert_eq!(
            cfg.data.vanilla_data_dir,
            Some(PathBuf::from("data/vanilla"))
        );
    }

    #[test]
    fn parses_tellus_like_worldgen_config_without_changing_defaults() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "0.0.0.0"
            port = 25565

            [data]
            worldgen_mode = "tellus_like"
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
        assert_eq!(cfg.data.worldgen_mode, WorldgenMode::TellusLike);
    }

    #[test]
    fn data_section_rejects_removed_vanilla_dir() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "0.0.0.0"
            port = 25565

            [data]
            vanilla_dir = "data/vanilla"
        "#;

        let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
        assert!(err.to_string().contains("unknown field `vanilla_dir`"));
    }

    #[test]
    fn server_section_rejects_unknown_fields() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"
            online_mode = false

            [network]
            bind_address = "127.0.0.1"
            port = 25565
        "#;

        let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
        assert!(err.to_string().contains("unknown field `online_mode`"));
    }

    #[test]
    fn network_section_rejects_unknown_fields() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565
            online_mode = false
        "#;

        let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
        assert!(err.to_string().contains("unknown field `online_mode`"));
    }

    #[test]
    fn chunk_pipeline_rejects_removed_worker_percentages() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565

            [chunk_pipeline]
            chunk_worker_threads_percent = 75
        "#;

        let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
        assert!(
            err.to_string()
                .contains("unknown field `chunk_worker_threads_percent`")
        );
    }

    #[test]
    fn admin_section_rejects_unknown_fields() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565

            [admin]
            operators = []
            op_everyone = true
        "#;

        let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
        assert!(err.to_string().contains("unknown field `op_everyone`"));
    }

    #[test]
    fn auth_section_rejects_unknown_fields() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "127.0.0.1"
            port = 25565

            [auth]
            online_mode = false
            ops = ["Notch"]
        "#;

        let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
        assert!(err.to_string().contains("unknown field `ops`"));
    }

    #[test]
    fn parses_chunk_pipeline_overrides() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "0.0.0.0"
            port = 25565

            [chunk_pipeline]
            chunk_send_rate = 12
            chunk_load_rate = 8
            chunk_generate_rate = 4
            chunk_prepare_budget_ms = 3
            chunk_prepare_batch_size = 2
            chunk_result_queue_size = 9
            region_cache_size = 7
            compression_threshold = 128
            compression_level = 6
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
        assert_eq!(cfg.chunk_pipeline.chunk_send_rate, 12);
        assert_eq!(cfg.chunk_pipeline.chunk_load_rate, 8);
        assert_eq!(cfg.chunk_pipeline.chunk_generate_rate, 4);
        assert_eq!(cfg.chunk_pipeline.chunk_prepare_budget_ms, 3);
        assert_eq!(cfg.chunk_pipeline.chunk_prepare_batch_size, 2);
        assert_eq!(cfg.chunk_pipeline.chunk_result_queue_size, 9);
        assert_eq!(cfg.chunk_pipeline.region_cache_size, 7);
        assert_eq!(cfg.chunk_pipeline.compression_threshold, 128);
        assert_eq!(cfg.chunk_pipeline.compression_level, Some(6));
    }

    #[test]
    fn chunk_pipeline_normalizes_zero_values_for_runtime() {
        let section = ChunkPipelineSection {
            chunk_send_rate: 0,
            chunk_load_rate: 0,
            chunk_generate_rate: 0,
            chunk_prepare_budget_ms: 0,
            chunk_prepare_batch_size: 0,
            chunk_result_queue_size: 0,
            region_cache_size: 0,
            compression_threshold: -1,
            compression_level: Some(99),
        };
        let policy = section.to_network();
        assert_eq!(policy.chunk_send_rate, 1);
        assert_eq!(policy.chunk_load_rate, 1);
        assert_eq!(policy.chunk_generate_rate, 1);
        assert_eq!(policy.chunk_prepare_batch_size, 1);
        let defaults = mc_net::ChunkPipelinePolicy::default();
        assert_eq!(policy.chunk_io_threads, defaults.chunk_io_threads);
        assert_eq!(policy.chunk_worker_threads, defaults.chunk_worker_threads);
        assert_eq!(policy.chunk_result_queue_size, 1);
        assert_eq!(policy.region_cache_size, 1);
        assert_eq!(policy.compression_threshold, 0);
        assert_eq!(policy.compression_level, Some(9));
    }

    #[test]
    fn parses_simulation_overrides() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "0.0.0.0"
            port = 25565

            [simulation]
            random_tick_speed = 7
            save_interval_ticks = 40
            spawn_monsters = false
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");

        assert_eq!(cfg.simulation.random_tick_speed, 7);
        assert_eq!(cfg.simulation.save_interval_ticks, 40);
        assert!(!cfg.simulation.spawn_monsters);
    }

    #[test]
    fn parses_autoscale_overrides_and_builds_bounded_policy() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"
            view_distance = 8

            [network]
            bind_address = "0.0.0.0"
            port = 25565

            [chunk_pipeline]
            chunk_send_rate = 8
            chunk_load_rate = 16
            chunk_generate_rate = 16

            [autoscale]
            enabled = true
            profile = "low_end"
            min_view_distance = 3
            max_view_distance = 6
            target_tick_ms = 45
            target_first_chunk_ms = 1200
            scale_down_after_ticks = 2
            scale_up_after_ticks = 7
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");
        let policy = cfg.autoscale.to_policy(&cfg.chunk_pipeline);
        let limits = cfg
            .autoscale
            .initial_limits(&cfg.server, &cfg.chunk_pipeline);

        assert!(cfg.autoscale.enabled);
        assert_eq!(cfg.autoscale.profile, AutoscaleProfile::LowEnd);
        assert_eq!(policy.min_view_distance, 3);
        assert_eq!(policy.max_view_distance, 6);
        assert_eq!(policy.target_tick_ms, 45);
        assert_eq!(policy.target_first_chunk_ms, 1200);
        assert_eq!(policy.scale_down_after_ticks, 2);
        assert_eq!(policy.scale_up_after_ticks, 7);
        assert_eq!(limits.view_distance, 6);
        assert_eq!(limits.chunk_send_rate, 8);
        assert_eq!(limits.chunk_load_rate, 16);
        assert_eq!(limits.chunk_generate_rate, 16);

        let net = cfg
            .to_network(
                Arc::new(mc_data::testing::stub()),
                stub_blocks(),
                None,
                stub_tags(),
                Arc::new(Vec::new()),
                Arc::new(LootTables::default()),
                None,
                Arc::new(ItemRegistry::default()),
                Arc::new(ItemFactsTable::default()),
                Arc::new(BlockFactsTable::default()),
                Arc::new(mc_data::entity_types::solaris_required_entity_types()),
                Arc::new(BiomeSpawnRules::default()),
            )
            .unwrap();
        let runtime = net
            .chunk_pipeline
            .runtime_control
            .expect("enabled autoscale wires runtime control");
        assert_eq!(runtime.policy, policy);
        assert_eq!(runtime.initial_limits, limits);
    }

    #[test]
    fn autoscale_section_rejects_unknown_fields() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "M"

            [network]
            bind_address = "0.0.0.0"
            port = 25565

            [autoscale]
            enabled = false
            unexpected = true
        "#;

        let err = toml::from_str::<ServerConfig>(toml_src).unwrap_err();
        assert!(err.to_string().contains("unknown field `unexpected`"));
    }

    #[test]
    fn simulation_normalizes_runtime_budget() {
        let section = SimulationSection {
            random_tick_speed: 0,
            save_interval_ticks: 0,
            spawn_monsters: false,
        };
        let policy = section.to_network(42, 5);

        assert_eq!(policy.simulation_distance, 5);
        assert_eq!(policy.random_tick_speed, 0);
        assert_eq!(policy.chunk_budget, 64);
        assert_eq!(policy.fluid_tick_budget, 256);
        assert_eq!(policy.save_interval_ticks, 1);
        assert!(!policy.spawn_monsters);
        assert_eq!(policy.seed, 42);
    }

    #[test]
    fn simulation_rejects_removed_manual_work_budgets() {
        let error = toml::from_str::<ServerConfig>(
            r#"
                [simulation]
                random_tick_chunk_budget = 11
                scheduled_fluid_tick_budget = 13
            "#,
        )
        .expect_err("runtime work budgets must belong to autoscale");

        let message = error.to_string();
        assert!(
            message.contains("random_tick_chunk_budget")
                || message.contains("scheduled_fluid_tick_budget")
        );
    }

    fn stub_blocks() -> Arc<BlockRegistry> {
        Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"))
    }

    fn stub_tags() -> Arc<TagsData> {
        Arc::new(TagsData::default())
    }

    #[test]
    fn translates_to_network_config() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = "Howdy"
            max_players = 50
            view_distance = 7
            simulation_distance = 5

            [network]
            bind_address = "127.0.0.1"
            port = 25000

            [data]
            world_dir = "/tmp/world"
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).unwrap();
        let data = Arc::new(mc_data::testing::stub());
        let net = cfg
            .to_network(
                data,
                stub_blocks(),
                None,
                stub_tags(),
                Arc::new(Vec::new()),
                Arc::new(LootTables::default()),
                None,
                Arc::new(ItemRegistry::default()),
                Arc::new(ItemFactsTable::default()),
                Arc::new(BlockFactsTable::default()),
                Arc::new(mc_data::entity_types::solaris_required_entity_types()),
                Arc::new(BiomeSpawnRules::default()),
            )
            .unwrap();
        assert_eq!(net.motd, "Howdy");
        assert_eq!(net.max_players, 50);
        assert_eq!(net.view_distance, 7);
        assert_eq!(net.random_tick.simulation_distance, 5);
        assert_eq!(net.bind_address.port(), 25000);
        assert!(net.world.is_none());
        assert_eq!(net.chunk_pipeline.region_cache_size, 4);
        let runtime = net
            .chunk_pipeline
            .runtime_control
            .expect("default config wires runtime autoscale");
        assert_eq!(runtime.initial_limits.view_distance, 7);
        assert_eq!(cfg.data.world_dir, Some(PathBuf::from("/tmp/world")));
    }

    #[test]
    fn invalid_bind_address_is_rejected() {
        let toml_src = r#"
            [server]
            name = "S"
            motd = ""

            [network]
            bind_address = "not-an-ip"
            port = 25565
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).unwrap();
        let data = Arc::new(mc_data::testing::stub());
        assert!(
            cfg.to_network(
                data,
                stub_blocks(),
                None,
                stub_tags(),
                Arc::new(Vec::new()),
                Arc::new(LootTables::default()),
                None,
                Arc::new(ItemRegistry::default()),
                Arc::new(ItemFactsTable::default()),
                Arc::new(BlockFactsTable::default()),
                Arc::new(mc_data::entity_types::solaris_required_entity_types()),
                Arc::new(BiomeSpawnRules::default())
            )
            .is_err()
        );
    }
}
