use std::sync::Arc;

use mc_data::Identifier;
use mc_data::biomes::BiomeSpawnRules;
use mc_data::block_facts::BlockFactsTable;
use mc_data::item_components::ItemFactsTable;
use mc_data::items::ItemRegistry;
use mc_data::loot::LootTables;
use mc_data::tags::TagsData;
use mc_world::{BlockRegistry, BlockStateId, Chunk, ChunkPos, WorldStorage};

fn config(min_y: i32, height: i32) -> mc_server::ServerConfig {
    toml::from_str(&format!(
        r#"
            [server]
            name = "geometry-test"
            motd = "geometry-test"

            [network]
            bind_address = "127.0.0.1"
            port = 0

            [data]
            min_y = {min_y}
            height = {height}
        "#
    ))
    .expect("parse test config")
}

fn resident_overworld_chunk(blocks: Arc<BlockRegistry>) -> mc_net::WorldHandle {
    let position = ChunkPos { x: 3, z: -2 };
    let chunk = Chunk::empty(
        position,
        BlockStateId(0),
        Identifier::parse("minecraft:plains").expect("valid biome identifier"),
    );
    let mut storage = WorldStorage::in_memory(blocks);
    storage
        .commit_chunk_snapshot(position, chunk)
        .expect("cache existing chunk");
    Arc::new(tokio::sync::Mutex::new(storage))
}

fn translate(
    config: &mc_server::ServerConfig,
    blocks: Arc<BlockRegistry>,
    world: mc_net::WorldHandle,
) -> anyhow::Result<mc_net::ServerConfig> {
    config.to_network(
        Arc::new(mc_data::testing::stub()),
        blocks,
        Some(world),
        Arc::new(TagsData::default()),
        Arc::new(Vec::new()),
        Arc::new(LootTables::default()),
        None,
        Arc::new(ItemRegistry::default()),
        Arc::new(ItemFactsTable::default()),
        Arc::new(BlockFactsTable::default()),
        Arc::new(mc_data::entity_types::solaris_required_entity_types()),
        Arc::new(BiomeSpawnRules::default()),
    )
}

#[test]
fn custom_geometry_rejects_loaded_overworld_chunk() {
    let blocks = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = resident_overworld_chunk(Arc::clone(&blocks));

    let error = match translate(&config(0, 256), blocks, world) {
        Ok(_) => panic!("custom geometry must reject an existing Overworld chunk"),
        Err(error) => error,
    };
    let message = error.to_string();
    assert!(message.contains("chunk (3, -2)"), "{message}");
    assert!(message.contains("-64..320"), "{message}");
    assert!(message.contains("0..256"), "{message}");
}

#[test]
fn overworld_geometry_accepts_loaded_overworld_chunk() {
    let blocks = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = resident_overworld_chunk(Arc::clone(&blocks));

    translate(&config(-64, 384), blocks, world).expect("matching geometry must start");
}
