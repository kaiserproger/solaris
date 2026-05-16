//! # mc-server
//!
//! Main server binary that ties the Solaris engine together.
//!
//! Part of the Solaris engine.

use std::net::{IpAddr, SocketAddr};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;

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
}

/// Identity-level server settings.
#[derive(Debug, Serialize, Deserialize)]
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
pub struct NetworkSection {
    pub bind_address: String,
    pub port: u16,
}

/// Where the vanilla data sidecar lives on disk. Defaults to
/// `./data/vanilla` relative to the working directory the server is
/// launched from, which matches the layout `tools/extract-vanilla-data.sh`
/// produces.
///
/// `world_dir` is the on-disk world save the server reads chunks from
/// at runtime. `None` (or the default) means "no world wired up yet";
/// the server will start and log a warning, and chunk queries will
/// resolve to `None` everywhere. Plumbing the world into the network
/// layer happens in M3.
#[derive(Debug, Serialize, Deserialize)]
pub struct DataSection {
    #[serde(default = "default_vanilla_dir")]
    pub vanilla_dir: PathBuf,
    #[serde(default)]
    pub world_dir: Option<PathBuf>,
    /// World seed for the M7 terrain generator. Defaults to `0` —
    /// every run starts on the same terrain unless this is overridden.
    /// Operators bumping this between runs will see fresh terrain in
    /// previously-unflushed chunks; chunks already written to `.mca`
    /// keep their old contents (the on-disk slot wins).
    #[serde(default)]
    pub seed: i64,
}

/// Chunk preparation, worker, and cache policy. M13 moves chunk work out
/// of the Play socket task in stages; these settings are the stable
/// operator-facing surface for that pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    #[serde(default = "default_chunk_io_threads_percent")]
    pub chunk_io_threads_percent: u32,
    #[serde(default = "default_chunk_worker_threads_percent")]
    pub chunk_worker_threads_percent: u32,
    #[serde(default = "default_entity_worker_threads_percent")]
    pub entity_worker_threads_percent: u32,
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
pub struct SimulationSection {
    #[serde(default = "default_random_tick_speed")]
    pub random_tick_speed: u32,
    #[serde(default = "default_random_tick_chunk_budget")]
    pub random_tick_chunk_budget: usize,
    #[serde(default = "default_scheduled_fluid_tick_budget")]
    pub scheduled_fluid_tick_budget: usize,
    #[serde(default = "default_save_interval_ticks")]
    pub save_interval_ticks: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminSection {
    #[serde(default)]
    pub operators: Vec<String>,
    #[serde(default = "default_allow_local_dev_operators")]
    pub allow_local_dev_operators: bool,
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
            random_tick_chunk_budget: policy.chunk_budget,
            scheduled_fluid_tick_budget: policy.fluid_tick_budget,
            save_interval_ticks: policy.save_interval_ticks,
        }
    }
}

impl SimulationSection {
    #[must_use]
    pub fn to_network(&self, seed: i64) -> mc_net::RandomTickPolicy {
        mc_net::RandomTickPolicy {
            random_tick_speed: self.random_tick_speed,
            chunk_budget: self.random_tick_chunk_budget.max(1),
            fluid_tick_budget: self.scheduled_fluid_tick_budget.max(1),
            save_interval_ticks: self.save_interval_ticks.max(1),
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
            chunk_io_threads_percent: default_chunk_io_threads_percent(),
            chunk_worker_threads_percent: default_chunk_worker_threads_percent(),
            entity_worker_threads_percent: default_entity_worker_threads_percent(),
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
        let cores = available_parallelism();
        mc_net::ChunkPipelinePolicy {
            chunk_send_rate: self.chunk_send_rate.max(1),
            chunk_load_rate: self.chunk_load_rate.max(1),
            chunk_generate_rate: self.chunk_generate_rate.max(1),
            chunk_prepare_budget_ms: self.chunk_prepare_budget_ms,
            chunk_prepare_batch_size: self.chunk_prepare_batch_size.max(1),
            chunk_io_threads: threads_from_percent(cores, self.chunk_io_threads_percent),
            chunk_worker_threads: threads_from_percent(cores, self.chunk_worker_threads_percent),
            entity_worker_threads: threads_from_percent(cores, self.entity_worker_threads_percent),
            chunk_result_queue_size: self.chunk_result_queue_size.max(1),
            region_cache_size: self.region_cache_size.max(1),
            compression_threshold: self.compression_threshold.max(0),
            compression_level: self.compression_level.map(|level| level.min(9)),
        }
    }
}

fn available_parallelism() -> usize {
    std::thread::available_parallelism()
        .map(NonZeroUsize::get)
        .unwrap_or(1)
}

fn threads_from_percent(cores: usize, percent: u32) -> usize {
    let cores = cores.max(1);
    let scaled = cores.saturating_mul(percent as usize).div_ceil(100);
    scaled.max(1)
}

impl Default for DataSection {
    fn default() -> Self {
        Self {
            vanilla_dir: default_vanilla_dir(),
            world_dir: None,
            seed: 0,
        }
    }
}

fn default_max_players() -> u32 {
    20
}

fn default_view_distance() -> i32 {
    mc_net::DEFAULT_VIEW_DISTANCE
}

fn default_vanilla_dir() -> PathBuf {
    PathBuf::from("data/vanilla")
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

fn default_chunk_io_threads_percent() -> u32 {
    25
}

fn default_chunk_worker_threads_percent() -> u32 {
    50
}

fn default_entity_worker_threads_percent() -> u32 {
    25
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

fn default_random_tick_chunk_budget() -> usize {
    mc_net::RandomTickPolicy::default().chunk_budget
}

fn default_scheduled_fluid_tick_budget() -> usize {
    mc_net::RandomTickPolicy::default().fluid_tick_budget
}

fn default_save_interval_ticks() -> u64 {
    mc_net::RandomTickPolicy::default().save_interval_ticks
}

fn default_allow_local_dev_operators() -> bool {
    true
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
    ) -> Result<mc_net::ServerConfig, std::net::AddrParseError> {
        let ip: IpAddr = self.network.bind_address.parse()?;
        Ok(mc_net::ServerConfig {
            bind_address: SocketAddr::new(ip, self.network.port),
            motd: self.server.motd.clone(),
            max_players: self.server.max_players,
            view_distance: self.server.view_distance.max(0),
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
            chunk_pipeline: self.chunk_pipeline.to_network(),
            random_tick: self.simulation.to_network(self.data.seed),
            command_permissions: mc_net::CommandPermissionConfig::new(
                self.admin.operators.clone(),
                self.admin.allow_local_dev_operators,
            ),
            shutdown: mc_net::ShutdownHandle::default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(cfg.data.vanilla_dir, PathBuf::from("data/vanilla"));
        assert_eq!(cfg.chunk_pipeline.chunk_prepare_batch_size, 8);
        assert_eq!(cfg.chunk_pipeline.chunk_io_threads_percent, 25);
        assert_eq!(cfg.chunk_pipeline.chunk_worker_threads_percent, 50);
        assert_eq!(cfg.chunk_pipeline.entity_worker_threads_percent, 25);
        assert_eq!(cfg.simulation.random_tick_speed, 3);
        assert_eq!(cfg.simulation.random_tick_chunk_budget, 64);
        assert_eq!(cfg.simulation.scheduled_fluid_tick_budget, 256);
        assert_eq!(cfg.simulation.save_interval_ticks, 20);
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
            chunk_io_threads_percent = 25
            chunk_worker_threads_percent = 75
            entity_worker_threads_percent = 30
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
        assert_eq!(cfg.chunk_pipeline.chunk_io_threads_percent, 25);
        assert_eq!(cfg.chunk_pipeline.chunk_worker_threads_percent, 75);
        assert_eq!(cfg.chunk_pipeline.entity_worker_threads_percent, 30);
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
            chunk_io_threads_percent: 0,
            chunk_worker_threads_percent: 0,
            entity_worker_threads_percent: 0,
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
        assert_eq!(policy.chunk_io_threads, 1);
        assert_eq!(policy.chunk_worker_threads, 1);
        assert_eq!(policy.entity_worker_threads, 1);
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
            random_tick_chunk_budget = 11
            scheduled_fluid_tick_budget = 13
            save_interval_ticks = 40
        "#;
        let cfg: ServerConfig = toml::from_str(toml_src).expect("parse");

        assert_eq!(cfg.simulation.random_tick_speed, 7);
        assert_eq!(cfg.simulation.random_tick_chunk_budget, 11);
        assert_eq!(cfg.simulation.scheduled_fluid_tick_budget, 13);
        assert_eq!(cfg.simulation.save_interval_ticks, 40);
    }

    #[test]
    fn simulation_normalizes_runtime_budget() {
        let section = SimulationSection {
            random_tick_speed: 0,
            random_tick_chunk_budget: 0,
            scheduled_fluid_tick_budget: 0,
            save_interval_ticks: 0,
        };
        let policy = section.to_network(42);

        assert_eq!(policy.random_tick_speed, 0);
        assert_eq!(policy.chunk_budget, 1);
        assert_eq!(policy.fluid_tick_budget, 1);
        assert_eq!(policy.save_interval_ticks, 1);
        assert_eq!(policy.seed, 42);
    }

    #[test]
    fn chunk_pool_percentages_scale_from_available_cores() {
        assert_eq!(threads_from_percent(3, 25), 1);
        assert_eq!(threads_from_percent(3, 50), 2);
        assert_eq!(threads_from_percent(8, 25), 2);
        assert_eq!(threads_from_percent(8, 50), 4);
        assert_eq!(threads_from_percent(8, 0), 1);
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
            vanilla_dir = "/tmp/vanilla"
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
                Arc::new(EntityTypeRegistry::default()),
                Arc::new(BiomeSpawnRules::default()),
            )
            .unwrap();
        assert_eq!(net.motd, "Howdy");
        assert_eq!(net.max_players, 50);
        assert_eq!(net.view_distance, 7);
        assert_eq!(cfg.server.simulation_distance, 5);
        assert_eq!(net.bind_address.port(), 25000);
        assert!(net.world.is_none());
        assert_eq!(net.chunk_pipeline.region_cache_size, 4);
        assert_eq!(cfg.data.vanilla_dir, PathBuf::from("/tmp/vanilla"));
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
                Arc::new(EntityTypeRegistry::default()),
                Arc::new(BiomeSpawnRules::default())
            )
            .is_err()
        );
    }
}
