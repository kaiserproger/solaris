use super::*;

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
    pub scale_down_after_seconds: Option<u32>,
    #[serde(default)]
    pub scale_up_after_seconds: Option<u32>,
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
            scale_down_after_seconds: None,
            scale_up_after_seconds: None,
        }
    }
}

impl AutoscaleSection {
    #[must_use]
    pub fn to_policy(
        &self,
        server: &ServerSection,
        chunk_pipeline: &ChunkPipelineSection,
    ) -> mc_net::AutoscalePolicy {
        let mut policy = mc_net::AutoscalePolicy::for_profile(self.profile.to_network());
        policy.max_view_distance = self.max_view_distance.unwrap_or(server.view_distance);
        policy.min_view_distance = self.min_view_distance.unwrap_or(
            policy
                .min_view_distance
                .min(server.view_distance)
                .min(policy.max_view_distance),
        );
        if let Some(value) = self.target_tick_ms {
            policy.target_tick_ms = value;
        }
        if let Some(value) = self.target_first_chunk_ms {
            policy.target_first_chunk_ms = value;
        }
        if let Some(value) = self.scale_down_after_seconds {
            policy.scale_down_after_seconds = value;
        }
        if let Some(value) = self.scale_up_after_seconds {
            policy.scale_up_after_seconds = value;
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
        .bounded(self.to_policy(server, chunk_pipeline))
    }
}

impl Default for AdminSection {
    fn default() -> Self {
        Self {
            operators: Vec::new(),
            operators_file: None,
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
            friendly_spawn_interval_ticks: policy.friendly_spawn_interval_ticks,
            hostile_spawn_interval_ticks: policy.hostile_spawn_interval_ticks,
            friendly_spawn_chunk_budget: policy.friendly_spawn_chunk_budget,
            hostile_spawn_chunk_budget: policy.hostile_spawn_chunk_budget,
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
            friendly_spawn_interval_ticks: self.friendly_spawn_interval_ticks,
            hostile_spawn_interval_ticks: self.hostile_spawn_interval_ticks,
            friendly_spawn_chunk_budget: self.friendly_spawn_chunk_budget,
            hostile_spawn_chunk_budget: self.hostile_spawn_chunk_budget,
            seed: seed as u64,
        }
        .normalized()
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
            worker_threads: 0,
        }
    }
}

impl ChunkPipelineSection {
    #[must_use]
    pub fn to_network(&self) -> mc_net::ChunkPipelinePolicy {
        let worker_defaults = mc_net::ChunkPipelinePolicy::default();
        // An explicit bound replaces the derived split; one IO thread is kept so
        // region reads and writes are never serialized behind chunk work.
        let (chunk_io_threads, chunk_worker_threads) = if self.worker_threads == 0 {
            (
                worker_defaults.chunk_io_threads,
                worker_defaults.chunk_worker_threads,
            )
        } else {
            (1, self.worker_threads.max(1))
        };
        mc_net::ChunkPipelinePolicy {
            chunk_send_rate: self.chunk_send_rate.max(1),
            chunk_load_rate: self.chunk_load_rate.max(1),
            chunk_generate_rate: self.chunk_generate_rate.max(1),
            chunk_prepare_budget_ms: self.chunk_prepare_budget_ms,
            chunk_prepare_batch_size: self.chunk_prepare_batch_size.max(1),
            chunk_io_threads,
            chunk_worker_threads,
            chunk_result_queue_size: self.chunk_result_queue_size.max(1),
            region_cache_size: self.region_cache_size.max(1),
            compression_threshold: self.compression_threshold.max(0),
            compression_level: self.compression_level.map(|level| level.min(9)),
            runtime_control: None,
        }
    }
}

pub(super) fn default_max_players() -> u32 {
    20
}

pub(super) fn default_view_distance() -> i32 {
    mc_net::DEFAULT_VIEW_DISTANCE
}

pub(super) fn default_dimension_min_y() -> i32 {
    mc_world::MIN_Y
}

pub(super) fn default_dimension_height() -> i32 {
    mc_world::MAX_Y - mc_world::MIN_Y
}

pub(super) fn default_chunk_send_rate() -> u32 {
    mc_net::ChunkPipelinePolicy::default().chunk_send_rate
}

pub(super) fn default_chunk_load_rate() -> u32 {
    mc_net::ChunkPipelinePolicy::default().chunk_load_rate
}

pub(super) fn default_chunk_generate_rate() -> u32 {
    mc_net::ChunkPipelinePolicy::default().chunk_generate_rate
}

pub(super) fn default_chunk_prepare_batch_size() -> usize {
    mc_net::ChunkPipelinePolicy::default().chunk_prepare_batch_size
}

pub(super) fn default_chunk_result_queue_size() -> usize {
    mc_net::ChunkPipelinePolicy::default().chunk_result_queue_size
}

pub(super) fn default_region_cache_size() -> usize {
    mc_net::ChunkPipelinePolicy::default().region_cache_size
}

pub(super) fn default_compression_threshold() -> i32 {
    mc_net::ChunkPipelinePolicy::default().compression_threshold
}

pub(super) fn default_random_tick_speed() -> u32 {
    mc_net::RandomTickPolicy::default().random_tick_speed
}

pub(super) fn default_save_interval_ticks() -> u64 {
    mc_net::RandomTickPolicy::default().save_interval_ticks
}

pub(super) fn default_friendly_spawn_interval_ticks() -> u64 {
    mc_net::RandomTickPolicy::default().friendly_spawn_interval_ticks
}

pub(super) fn default_hostile_spawn_interval_ticks() -> u64 {
    mc_net::RandomTickPolicy::default().hostile_spawn_interval_ticks
}

pub(super) fn default_friendly_spawn_chunk_budget() -> usize {
    mc_net::RandomTickPolicy::default().friendly_spawn_chunk_budget
}

pub(super) fn default_hostile_spawn_chunk_budget() -> usize {
    mc_net::RandomTickPolicy::default().hostile_spawn_chunk_budget
}

pub(super) fn default_allow_local_dev_operators() -> bool {
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
                policy: self.autoscale.to_policy(&self.server, &self.chunk_pipeline),
                initial_limits: self
                    .autoscale
                    .initial_limits(&self.server, &self.chunk_pipeline),
            });
        }
        Ok(mc_net::ServerConfig {
            bind_address: SocketAddr::new(ip, self.network.port),
            motd: self.server.motd.clone(),
            max_players: self.server.max_players,
            tab_list: mc_net::TabListConfig {
                header: self.tab_list.header.clone(),
                footer: self.tab_list.footer.clone(),
            },
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
            loader_manifest: None,
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
