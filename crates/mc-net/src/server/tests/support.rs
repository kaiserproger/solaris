use super::super::*;
use mc_data::blocks::{BlockReport, BlockStateReport};
use std::collections::BTreeMap;

pub(super) type StateSpec<'a> = (u32, bool, &'a [(&'a str, &'a str)]);

pub(super) fn canonical_entity_types() -> Arc<EntityTypeRegistry> {
    Arc::new(mc_data::entity_types::solaris_required_entity_types())
}

pub(super) fn report(id: &str, props: &[(&str, &[&str])], states: &[StateSpec<'_>]) -> BlockReport {
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

pub(super) fn state_id(
    blocks: &[BlockReport],
    block_name: &str,
    properties: &[(&str, &str)],
) -> u32 {
    let block = blocks
        .iter()
        .find(|block| block.id.as_str() == block_name)
        .unwrap_or_else(|| panic!("missing block {block_name}"));
    block
        .states
        .iter()
        .find(|state| {
            properties.iter().all(|(name, value)| {
                state.properties.get(*name).map(String::as_str) == Some(*value)
            })
        })
        .unwrap_or_else(|| panic!("missing state for {block_name}: {properties:?}"))
        .id
}

pub(super) fn isolated_oak_fence_physics_world() -> (WorldStorage, BlockMaterialIds) {
    let reports = mc_data::blocks::solaris_required_blocks_report();
    let air = state_id(&reports, "minecraft:air", &[]);
    let fence = state_id(
        &reports,
        "minecraft:oak_fence",
        &[
            ("east", "false"),
            ("north", "false"),
            ("south", "false"),
            ("west", "false"),
            ("waterlogged", "false"),
        ],
    );
    let registry = Arc::new(BlockRegistry::from_report(&reports).unwrap());
    let facts = BlockFactsTable::from_blocks_report(&reports);
    let materials = material_ids(&registry, &facts);
    let mut storage = WorldStorage::in_memory(registry);
    let chunk_pos = mc_world::ChunkPos { x: 0, z: 0 };
    let mut chunk = mc_world::Chunk::empty(
        chunk_pos,
        mc_world::BlockStateId(air),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    let _ = chunk.set_block(9, 64, 8, mc_world::BlockStateId(fence));
    storage.insert_generated_chunk(chunk_pos, chunk).unwrap();
    (storage, materials)
}

pub(super) struct FlatGenerator {
    pub(super) air: mc_world::BlockStateId,
    pub(super) ground: mc_world::BlockStateId,
    pub(super) biome: Identifier,
}

impl mc_world::ChunkGenerator for FlatGenerator {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        let mut chunk = mc_world::Chunk::empty(pos, self.air, self.biome.clone());
        for x in 0..mc_world::SECTION_DIM as u8 {
            for z in 0..mc_world::SECTION_DIM as u8 {
                let _ = chunk.set_block(x, 63, z, self.ground);
            }
        }
        chunk
    }
}

pub(super) fn collect_collision_boxes(
    sampler: &SampledPhysicsWorld,
    x: i32,
    y: i32,
    z: i32,
) -> Vec<BlockCollisionBox> {
    let mut boxes = Vec::new();
    sampler.collision_boxes_at(x, y, z, &mut |collision_box| boxes.push(collision_box));
    boxes
}

pub(super) fn collision_route_samplers(
    storage: &WorldStorage,
    blocks: &Arc<BlockRegistry>,
    materials: &BlockMaterialIds,
    chunk_pos: mc_world::ChunkPos,
) -> (SampledPhysicsWorld, SampledPhysicsWorld) {
    let chunks = HashMap::from([(chunk_pos, storage.cached_chunk_snapshot(chunk_pos))]);
    let make_sampler = |compatible: bool| {
        SampledPhysicsWorld::without_entity_context(Arc::new(EntityPhysicsSnapshot {
            chunks: chunks.clone(),
            materials: Arc::new(materials.clone()),
            blocks: Some(Arc::clone(blocks)),
            powder_snow_states: powder_snow_state_ids(blocks),
            collision_direct_lookup_compatible: compatible,
        }))
    };
    (make_sampler(true), make_sampler(false))
}

pub(crate) fn save_all_test_config(
    tmp: &std::path::Path,
    blocks: Arc<BlockRegistry>,
    items: Arc<mc_data::items::ItemRegistry>,
    entity_types: Arc<mc_data::entity_types::EntityTypeRegistry>,
) -> ServerConfig {
    let world = Arc::new(Mutex::new(
        WorldStorage::open(tmp, Arc::clone(&blocks))
            .unwrap()
            .with_item_registry(Arc::clone(&items)),
    ));
    ServerConfig {
        tab_list: crate::server::TabListConfig::default(),
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
        items,
        item_facts: Arc::new(mc_data::item_components::ItemFactsTable::default()),
        block_facts: Arc::new(mc_data::block_facts::BlockFactsTable::default()),
        entity_types,
        biome_spawns: Arc::new(mc_data::biomes::BiomeSpawnRules::default()),
        chunk_pipeline: ChunkPipelinePolicy::default(),
        random_tick: play::RandomTickPolicy::default(),
        command_permissions: CommandPermissionConfig::new(Vec::<String>::new(), true),
        loader_manifest: None,
        shutdown: ShutdownHandle::default(),
    }
}

pub(super) fn access_control_handle(config: &CommandPermissionConfig) -> OperatorControlHandle {
    OperatorControlHandle {
        sessions: Arc::new(play::SessionRegistry::new()),
        simulation: play::simulation_channel().0,
        shutdown: ShutdownHandle::default(),
        runtime_control: None,
        resources: ChunkPipelineResources::with_limits(1, 1),
        operators: config.operator_identities(),
        whitelist: config.whitelist_identities(),
    }
}
