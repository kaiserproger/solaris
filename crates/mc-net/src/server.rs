//! TCP listener, accept loop, and the per-connection state-machine entry
//! point.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::BytesMut;
use mc_data::Identifier;
use mc_data::VanillaData;
use mc_data::biomes::BiomeSpawnRules;
use mc_data::block_facts::BlockFactsTable;
use mc_data::block_light::BlockLightTable;
use mc_data::entity_types::EntityTypeRegistry;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_data::loot::LootTables;
use mc_data::recipes::Recipe;
use mc_data::tags::TagsData;
use mc_physics::{BlockMaterial, BlockMaterialIds, BlockSampler, EntityBody, PhysicsConfig};
use mc_protocol::State;
use mc_protocol::frame::Compression;
use mc_protocol::packets::handshake::{Handshake, NextState};
use mc_world::{BlockPos, BlockRegistry, WorldStorage};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Semaphore};
use tracing::{debug, info, warn};

use crate::ChunkPipelinePolicy;
use crate::connection::read_packet;
use crate::error::ConnectionError;
use crate::{configuration, login, play, status};

/// Shared, mutably-accessible handle to the world.
///
/// `WorldStorage::get_chunk` is `&mut self` — it touches an internal
/// LRU on every call — so we wrap it in a tokio Mutex. The mutex is
/// async because chunk reads will eventually await disk I/O (M3.f's
/// region cache + worker pool).
pub type WorldHandle = Arc<Mutex<WorldStorage>>;

/// Settings the network layer needs to serve.
///
/// Constructed by the caller (`mc-server`) from the user-facing TOML
/// config so the network layer does not depend on the binary's config
/// types. `data` is the in-memory index of vanilla registries the
/// Configuration state hands back to clients. `blocks` is the block-
/// state registry built from `blocks.json` — needed by the chunk-data
/// path in M3+; held even when no `world` is configured. `world` is
/// the open world handle when `[data].world_dir` is set; `None` keeps
/// the M1-style chunkless Play state intact.
///
/// `Debug` is not derived: neither `BlockRegistry` nor `WorldStorage`
/// implements `Debug`. Connection-scope logging uses individual fields
/// instead of `{:?}`-printing the whole config.
#[derive(Clone)]
pub struct ServerConfig {
    pub bind_address: SocketAddr,
    pub motd: String,
    pub max_players: u32,
    pub view_distance: i32,
    pub data: Arc<VanillaData>,
    pub blocks: Arc<BlockRegistry>,
    pub world: Option<WorldHandle>,
    /// Tag set the Configuration handler ships in `UpdateTags` between
    /// the last `RegistryData` and `FinishConfiguration`. May be the
    /// empty default when the sidecar lacks tag JSON; the vanilla
    /// client then complains during registry freeze.
    pub tags: Arc<TagsData>,
    pub recipes: Arc<Vec<Recipe>>,
    pub loot: Arc<LootTables>,
    /// Per-block-state light metadata (emission / opacity /
    /// sky-propagation). Built by `mc-server` from the required block
    /// report at startup; the chunk-streaming path uses it to compute
    /// light when the Anvil nibbles are missing. `None` is kept for
    /// narrow protocol tests that do not exercise chunk lighting.
    pub block_light: Option<Arc<BlockLightTable>>,
    /// Item registry (M6.c) loaded from
    /// `data/vanilla/reports/registries.json`. Drives the M6
    /// place-from-held-item lookup. May be empty when running tests
    /// that don't care about inventory; the M6 place flow degrades
    /// gracefully (no item → no placement).
    pub items: Arc<ItemRegistry>,
    pub item_facts: Arc<ItemFactsTable>,
    pub block_facts: Arc<BlockFactsTable>,
    pub entity_types: Arc<EntityTypeRegistry>,
    pub biome_spawns: Arc<BiomeSpawnRules>,
    /// M13 chunk-pipeline policy. Early M13 slices keep the existing
    /// cooperative stream path but thread this policy through so the
    /// scheduler and worker-pool stages have one runtime source of truth.
    pub chunk_pipeline: ChunkPipelinePolicy,
    pub random_tick: play::RandomTickPolicy,
}

/// A listener that has been successfully bound but is not yet serving.
///
/// Holding the listener and the accept loop in two distinct steps lets
/// callers (including the integration tests) learn the assigned port
/// when binding to `0.0.0.0:0` *without* a probe/drop/rebind dance that
/// races against the OS reusing the same ephemeral port.
pub struct BoundServer {
    listener: TcpListener,
    config: Arc<ServerConfig>,
    sessions: Arc<play::SessionRegistry>,
}

#[derive(Debug, Clone)]
pub struct SaveAllReport {
    pub players_saved: usize,
    pub entities_saved: usize,
    pub chunks_flushed: usize,
    pub world_metadata_saved: bool,
    pub errors: Vec<String>,
}

impl SaveAllReport {
    #[must_use]
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

#[derive(Clone)]
pub struct SaveHandle {
    config: Arc<ServerConfig>,
    sessions: Arc<play::SessionRegistry>,
}

impl SaveHandle {
    pub async fn save_all(&self) -> SaveAllReport {
        save_all(&self.config, &self.sessions).await
    }
}

impl BoundServer {
    /// The socket address the listener is bound to.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    #[must_use]
    pub fn save_handle(&self) -> SaveHandle {
        SaveHandle {
            config: Arc::clone(&self.config),
            sessions: Arc::clone(&self.sessions),
        }
    }

    /// Accept connections forever, spawning a per-connection task each
    /// time. An error inside a connection task is logged but does not
    /// stop the listener.
    pub async fn serve(self) -> std::io::Result<()> {
        info!(
            addr = %self.local_addr()?,
            registries = self.config.data.registry_count(),
            entries = self.config.data.entry_count(),
            "Solaris is listening"
        );
        let config = self.config;
        let sessions = self.sessions;
        let entity_world_root = if let Some(world) = config.world.as_ref() {
            let storage = world.lock().await;
            storage.world_root().map(std::path::Path::to_path_buf)
        } else {
            None
        };
        let entity_sessions = Arc::clone(&sessions);
        let entity_config = Arc::clone(&config);
        let entity_cpu_permits =
            Arc::new(Semaphore::new(config.chunk_pipeline.entity_worker_threads));
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(play::ENTITY_TICK_PERIOD);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut tick = 0_u64;
            loop {
                ticker.tick().await;
                tick = tick.wrapping_add(1);
                let world_time = entity_sessions.advance_world_time(1);
                let queries = entity_sessions.tick_entities_and_collect_physics_queries(tick);
                let steps =
                    entity_physics_steps(&entity_config, Arc::clone(&entity_cpu_permits), &queries)
                        .await;
                entity_sessions.apply_entity_physics_and_dispatch(tick, &steps);
                if tick.is_multiple_of(20)
                    && let Some(root) = entity_world_root.as_deref()
                {
                    let snapshots = entity_sessions.persisted_entity_snapshots();
                    if let Err(err) = play::persistence::save_persisted_entities(
                        root,
                        &entity_config.items,
                        &snapshots,
                    ) {
                        warn!(error = %err, "persisted entity save failed");
                    }
                    let metadata = play::persistence::WorldPersistedMetadata {
                        world_time,
                        world_identity: play::persistence::world_identity(root),
                    };
                    if let Err(err) = play::persistence::save_world_metadata(root, &metadata) {
                        warn!(error = %err, "world metadata save failed");
                    }
                }
                play::run_random_ticks(&entity_config, &entity_sessions, tick).await;
                play::run_scheduled_fluid_ticks(&entity_config, &entity_sessions, tick).await;
            }
        });
        loop {
            let (socket, peer) = self.listener.accept().await?;
            debug!(%peer, "accepted connection");
            let config = config.clone();
            let sessions = Arc::clone(&sessions);
            tokio::spawn(async move {
                if let Err(err) = handle_connection(socket, &config, sessions).await {
                    match err {
                        err if is_client_disconnect(&err) => {
                            debug!(%peer, "client disconnected");
                        }
                        other => {
                            warn!(%peer, error = %other, "connection terminated");
                        }
                    }
                } else {
                    debug!(%peer, "connection finished");
                }
            });
        }
    }
}

async fn entity_physics_steps(
    config: &ServerConfig,
    cpu_permits: Arc<Semaphore>,
    queries: &[play::EntityPhysicsQuery],
) -> Vec<play::EntityPhysicsStep> {
    let Some(world) = config.world.as_ref() else {
        return Vec::new();
    };
    if queries.is_empty() {
        return Vec::new();
    }
    let materials = material_ids(&config.blocks);
    let inputs = {
        let mut storage = world.lock().await;
        queries
            .iter()
            .map(|query| sample_entity_physics_input(*query, &mut storage, &materials))
            .collect::<Vec<_>>()
    };

    let workers = config
        .chunk_pipeline
        .entity_worker_threads
        .max(1)
        .min(inputs.len());
    let batch_size = inputs.len().div_ceil(workers);
    let mut handles = Vec::with_capacity(workers);
    for batch in inputs.chunks(batch_size) {
        let batch = batch.to_vec();
        let permit = match Arc::clone(&cpu_permits).acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => return Vec::new(),
        };
        handles.push(tokio::task::spawn_blocking(move || {
            let _permit = permit;
            batch
                .into_iter()
                .map(step_sampled_entity)
                .collect::<Vec<_>>()
        }));
    }

    let mut steps = Vec::with_capacity(queries.len());
    for handle in handles {
        match handle.await {
            Ok(mut batch) => steps.append(&mut batch),
            Err(err) if err.is_cancelled() => debug!("entity physics worker cancelled"),
            Err(err) => warn!(error = %err, "entity physics worker failed"),
        }
    }
    steps
}

#[derive(Clone)]
struct EntityPhysicsInput {
    query: play::EntityPhysicsQuery,
    samples: HashMap<BlockPos, BlockMaterial>,
}

struct SampledPhysicsWorld {
    samples: HashMap<BlockPos, BlockMaterial>,
}

impl BlockSampler for SampledPhysicsWorld {
    fn material_at(&mut self, x: i32, y: i32, z: i32) -> BlockMaterial {
        self.samples
            .get(&BlockPos { x, y, z })
            .copied()
            .unwrap_or(BlockMaterial::Air)
    }
}

fn sample_entity_physics_input(
    query: play::EntityPhysicsQuery,
    storage: &mut WorldStorage,
    materials: &BlockMaterialIds,
) -> EntityPhysicsInput {
    let mut samples = HashMap::new();
    for pos in entity_physics_sample_positions(query) {
        let material = storage
            .get_cached_block(pos)
            .map(|state| materials.classify(state.0))
            .unwrap_or(BlockMaterial::Air);
        samples.insert(pos, material);
    }
    EntityPhysicsInput { query, samples }
}

fn entity_physics_sample_positions(query: play::EntityPhysicsQuery) -> Vec<BlockPos> {
    let config = PhysicsConfig::default();
    let body = EntityBody {
        position: physics_vec(query.position),
        velocity: physics_vec(query.velocity),
        aabb: query.aabb,
        on_ground: query.on_ground,
    };
    let next_x = body.position.x + body.velocity.x * config.tick_seconds;
    let next_y = body.position.y + body.velocity.y * config.tick_seconds;
    let next_z = body.position.z + body.velocity.z * config.tick_seconds;
    let half = body.aabb.half_width;
    let min_x = (body.position.x.min(next_x) - half - 1.0).floor() as i32;
    let max_x = (body.position.x.max(next_x) + half + 1.0).floor() as i32;
    let min_z = (body.position.z.min(next_z) - half - 1.0).floor() as i32;
    let max_z = (body.position.z.max(next_z) + half + 1.0).floor() as i32;
    let min_y = (body.position.y.min(next_y) - 2.0).floor() as i32;
    let max_y = (body.position.y.max(next_y) + body.aabb.height + 2.0).floor() as i32;

    let mut positions = Vec::new();
    for x in min_x..=max_x {
        for y in min_y..=max_y {
            for z in min_z..=max_z {
                positions.push(BlockPos { x, y, z });
            }
        }
    }
    positions
}

fn step_sampled_entity(input: EntityPhysicsInput) -> play::EntityPhysicsStep {
    let mut sampler = SampledPhysicsWorld {
        samples: input.samples,
    };
    let result = mc_physics::step_entity(
        EntityBody {
            position: physics_vec(input.query.position),
            velocity: physics_vec(input.query.velocity),
            aabb: input.query.aabb,
            on_ground: input.query.on_ground,
        },
        &mut sampler,
        PhysicsConfig::default(),
    );
    play::EntityPhysicsStep {
        id: input.query.id,
        position: entity_vec(result.body.position),
        velocity: entity_vec(result.body.velocity),
        on_ground: result.body.on_ground,
    }
}

fn physics_vec(vec: mc_entity::Vec3) -> mc_physics::Vec3 {
    mc_physics::Vec3::new(vec.x, vec.y, vec.z)
}

fn entity_vec(vec: mc_physics::Vec3) -> mc_entity::Vec3 {
    mc_entity::Vec3::new(vec.x, vec.y, vec.z)
}

fn material_ids(blocks: &BlockRegistry) -> BlockMaterialIds {
    let state = |name: &str| {
        blocks
            .block(&Identifier::parse(name).expect("static identifier"))
            .map(|block| block.default.0)
    };
    let passable = crate::play::passive_entity_passable_blocks(blocks)
        .into_iter()
        .map(|state| state.0)
        .collect();

    BlockMaterialIds::new(
        state("minecraft:air").unwrap_or(0),
        state("minecraft:water"),
        state("minecraft:lava"),
    )
    .with_passable(passable)
}

fn is_client_disconnect(err: &ConnectionError) -> bool {
    match err {
        ConnectionError::Eof => true,
        ConnectionError::Io(err) => matches!(
            err.kind(),
            ErrorKind::BrokenPipe
                | ErrorKind::ConnectionAborted
                | ErrorKind::ConnectionReset
                | ErrorKind::UnexpectedEof
        ),
        _ => false,
    }
}

/// Bind to `config.bind_address` and return a [`BoundServer`] ready to
/// `.serve()`.
pub async fn bind(config: ServerConfig) -> std::io::Result<BoundServer> {
    let listener = TcpListener::bind(config.bind_address).await?;
    let sessions = Arc::new(play::SessionRegistry::new());
    if let Some(world) = config.world.as_ref() {
        let world_root = {
            let storage = world.lock().await;
            storage.world_root().map(std::path::Path::to_path_buf)
        };
        if let Some(root) = world_root.as_deref() {
            match play::persistence::load_world_metadata(root) {
                Ok(Some(metadata)) => {
                    let expected = play::persistence::world_identity(root);
                    if !metadata.world_identity.is_empty() && metadata.world_identity != expected {
                        warn!(
                            stored = %metadata.world_identity,
                            expected = %expected,
                            "world metadata identity mismatch"
                        );
                    }
                    sessions.set_world_time(metadata.world_time);
                    info!(world_time = metadata.world_time, "loaded world metadata");
                }
                Ok(None) => {}
                Err(err) => warn!(error = %err, "world metadata load failed"),
            }
            match play::persistence::load_persisted_entities(
                root,
                &config.items,
                &config.entity_types,
            ) {
                Ok(entities) => {
                    let restored = sessions.restore_persisted_entities(entities);
                    if restored > 0 {
                        info!(restored, "loaded persisted entities");
                    }
                }
                Err(err) => warn!(error = %err, "persisted entity load failed"),
            }
        }
    }
    Ok(BoundServer {
        listener,
        config: Arc::new(config),
        sessions,
    })
}

pub async fn save_all(config: &ServerConfig, sessions: &play::SessionRegistry) -> SaveAllReport {
    let mut report = SaveAllReport {
        players_saved: 0,
        entities_saved: 0,
        chunks_flushed: 0,
        world_metadata_saved: false,
        errors: Vec::new(),
    };
    let Some(world) = config.world.as_ref() else {
        return report;
    };
    let root = {
        let storage = world.lock().await;
        storage.world_root().map(std::path::Path::to_path_buf)
    };
    let Some(root) = root else {
        return report;
    };

    for (uuid, player) in sessions.persisted_player_states() {
        match play::persistence::save_player_state(&root, uuid, &config.items, &player) {
            Ok(()) => report.players_saved += 1,
            Err(err) => report
                .errors
                .push(format!("player {uuid}: save failed: {err}")),
        }
    }

    let entities = sessions.persisted_entity_snapshots();
    report.entities_saved = entities.len();
    if let Err(err) = play::persistence::save_persisted_entities(&root, &config.items, &entities) {
        report.errors.push(format!("entities: save failed: {err}"));
    }

    let metadata = play::persistence::WorldPersistedMetadata {
        world_time: sessions.world_time(),
        world_identity: play::persistence::world_identity(&root),
    };
    match play::persistence::save_world_metadata(&root, &metadata) {
        Ok(()) => report.world_metadata_saved = true,
        Err(err) => report
            .errors
            .push(format!("world metadata: save failed: {err}")),
    }

    let mut storage = world.lock().await;
    match storage.flush_dirty() {
        Ok(flushed) => report.chunks_flushed = flushed,
        Err(err) => report
            .errors
            .push(format!("dirty chunks: flush failed: {err}")),
    }

    report
}

/// Convenience for the binary: `bind` followed by `serve`.
pub async fn run(config: ServerConfig) -> std::io::Result<()> {
    bind(config).await?.serve().await
}

async fn handle_connection(
    socket: tokio::net::TcpStream,
    config: &ServerConfig,
    sessions: Arc<play::SessionRegistry>,
) -> Result<(), ConnectionError> {
    // Disable Nagle for low-latency interactive packets — same setting
    // vanilla uses.
    socket.set_nodelay(true)?;

    let (mut reader, mut writer) = socket.into_split();
    let mut buf = BytesMut::with_capacity(4096);
    let mut compression = Compression::Disabled;

    let handshake = read_packet::<Handshake, _>(
        &mut reader,
        &mut buf,
        Compression::Disabled,
        State::Handshake,
    )
    .await?;
    debug!(
        protocol = handshake.protocol_version,
        address = %handshake.server_address,
        port = handshake.server_port,
        next = ?handshake.next_state,
        "handshake received"
    );

    match handshake.next_state {
        NextState::Status => status::handle(&mut reader, &mut writer, &mut buf, config).await,
        NextState::Login | NextState::Transfer => {
            let profile = login::handle(
                &mut reader,
                &mut writer,
                &mut buf,
                &mut compression,
                config.chunk_pipeline.compression_level,
            )
            .await?;
            configuration::handle(
                &mut reader,
                &mut writer,
                &mut buf,
                compression,
                &profile,
                &config.data,
                &config.tags,
            )
            .await?;
            play::handle(
                &mut reader,
                &mut writer,
                &mut buf,
                compression,
                &profile,
                config,
                sessions,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_data::blocks::{BlockReport, BlockStateReport};
    use std::collections::BTreeMap;

    type StateSpec<'a> = (u32, bool, &'a [(&'a str, &'a str)]);

    fn report(id: &str, props: &[(&str, &[&str])], states: &[StateSpec<'_>]) -> BlockReport {
        BlockReport {
            id: Identifier::parse(id).unwrap(),
            properties: props
                .iter()
                .map(|(name, values)| {
                    (
                        (*name).to_string(),
                        values.iter().map(|value| (*value).to_string()).collect(),
                    )
                })
                .collect(),
            states: states
                .iter()
                .map(|(id, default, props)| BlockStateReport {
                    id: *id,
                    default: *default,
                    properties: props
                        .iter()
                        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                        .collect::<BTreeMap<_, _>>(),
                })
                .collect(),
        }
    }

    #[test]
    fn broken_pipe_is_graceful_disconnect() {
        let err = ConnectionError::Io(std::io::Error::from(ErrorKind::BrokenPipe));
        assert!(is_client_disconnect(&err));
    }

    #[test]
    fn codec_error_is_not_graceful_disconnect() {
        let err = ConnectionError::UnexpectedPacketId {
            state: State::Login,
            expected: 1,
            got: 2,
        };
        assert!(!is_client_disconnect(&err));
    }

    #[test]
    fn material_ids_treat_generated_vegetation_as_passable() {
        let reports = vec![
            report("minecraft:air", &[], &[(0, true, &[])]),
            report("minecraft:stone", &[], &[(1, true, &[])]),
            report("minecraft:water", &[], &[(2, true, &[])]),
            report("minecraft:lava", &[], &[(3, true, &[])]),
            report("minecraft:short_grass", &[], &[(4, true, &[])]),
            report("minecraft:poppy", &[], &[(5, true, &[])]),
            report(
                "minecraft:sugar_cane",
                &[("age", &["0", "1"])],
                &[(6, true, &[("age", "0")]), (7, false, &[("age", "1")])],
            ),
        ];
        let registry = BlockRegistry::from_report(&reports).unwrap();
        let ids = material_ids(&registry);

        assert_eq!(ids.classify(1), BlockMaterial::Solid);
        assert_eq!(ids.classify(2), BlockMaterial::Water);
        assert_eq!(ids.classify(3), BlockMaterial::Lava);
        assert_eq!(ids.classify(4), BlockMaterial::Air);
        assert_eq!(ids.classify(5), BlockMaterial::Air);
        assert_eq!(ids.classify(6), BlockMaterial::Air);
        assert_eq!(ids.classify(7), BlockMaterial::Air);
    }

    #[tokio::test]
    async fn save_all_writes_entities_and_world_metadata_to_real_storage() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("region")).unwrap();
        let blocks = Arc::new(BlockRegistry::from_report(&[]).unwrap());
        let items = Arc::new(mc_data::items::ItemRegistry::from_report(&[
            mc_data::items::ItemReport {
                id: Identifier::parse("minecraft:stone").unwrap(),
                protocol_id: 1,
            },
        ]));
        let entity_types = Arc::new(mc_data::entity_types::EntityTypeRegistry::from_report(&[
            mc_data::entity_types::EntityTypeReport {
                id: Identifier::parse("minecraft:item").unwrap(),
                protocol_id: 1,
            },
        ]));
        let world = Arc::new(Mutex::new(
            WorldStorage::open(tmp.path(), Arc::clone(&blocks))
                .unwrap()
                .with_item_registry(Arc::clone(&items)),
        ));
        let sessions = play::SessionRegistry::new();
        sessions.set_world_time(42);
        sessions.restore_persisted_entities([mc_entity::EntitySnapshot {
            id: mc_entity::EntityId(1_000_001),
            uuid: uuid::Uuid::from_u128(1),
            type_id: 1,
            type_name: "minecraft:item".into(),
            position: mc_entity::Vec3::new(1.0, 2.0, 3.0),
            rotation: mc_entity::Rotation::ZERO,
            velocity: mc_entity::Vec3::ZERO,
            on_ground: true,
            item_stack: Some(mc_entity::EntityItemStack::new(1, 2)),
            lifecycle: mc_entity::EntityLifecycle::Alive,
            health: 20.0,
            attributes: mc_entity::AttributeSet::vanilla_mob_defaults(),
            goal: mc_entity::GoalState::Idle,
        }]);
        let config = ServerConfig {
            bind_address: "127.0.0.1:0".parse().unwrap(),
            motd: "test".into(),
            max_players: 1,
            view_distance: 2,
            data: Arc::new(mc_data::testing::stub()),
            blocks,
            world: Some(world),
            tags: Arc::new(mc_data::tags::TagsData::default()),
            recipes: Arc::new(Vec::new()),
            loot: Arc::new(mc_data::loot::LootTables::default()),
            block_light: None,
            items: Arc::clone(&items),
            item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
            block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
            entity_types: Arc::clone(&entity_types),
            biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
            chunk_pipeline: ChunkPipelinePolicy::default(),
            random_tick: play::RandomTickPolicy::default(),
        };

        let report = save_all(&config, &sessions).await;

        assert!(report.is_ok(), "save-all errors: {:?}", report.errors);
        assert_eq!(report.entities_saved, 1);
        assert!(report.world_metadata_saved);
        let entities =
            play::persistence::load_persisted_entities(tmp.path(), &items, &entity_types).unwrap();
        assert_eq!(entities.len(), 1);
        assert_eq!(
            entities[0].item_stack,
            Some(mc_entity::EntityItemStack::new(1, 2))
        );
        let metadata = play::persistence::load_world_metadata(tmp.path())
            .unwrap()
            .unwrap();
        assert_eq!(metadata.world_time, 42);
    }
}
