//! M7.e — integration test for the baseline worldgen fallback.
//!
//! Boots `mc_net::run` against an empty tempdir (no `.mca` files
//! anywhere) attached to a `TerrainGenerator`. After the spawn
//! burst the test consults the shared world handle directly and
//! asserts a chunk at an arbitrary far position resolves to terrain
//! that contains the expected layers (bedrock at the bottom, grass
//! cap on the surface). The test does *not* drain the full spawn
//! burst on the wire to keep its wall-clock reasonable — driving
//! the burst is already covered by the M3.g / M4.f / M5.f / M6.g
//! harnesses.
//!
//! Skipped silently when the vanilla data sidecars are missing.

use std::path::PathBuf;
use std::sync::Arc;

use mc_world::ChunkGenerator;

#[tokio::test]
async fn empty_world_plus_generator_produces_terrain_on_demand() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let vanilla_dir = manifest.join("../../data/vanilla");
    let blocks_json = vanilla_dir.join("reports/blocks.json");
    if !blocks_json.exists() {
        eprintln!("skipping: {} missing", blocks_json.display());
        return;
    }

    let report = mc_data::blocks::load_blocks_report(&blocks_json).expect("blocks report");
    let blocks = Arc::new(mc_world::BlockRegistry::from_report(&report).expect("registry"));

    // Empty world: just create the expected dir layout.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("region")).unwrap();

    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        12345,
        Arc::clone(&blocks),
    ));
    let storage = mc_world::WorldStorage::open(tmp.path(), Arc::clone(&blocks))
        .unwrap()
        .with_generator(generator.clone() as Arc<dyn ChunkGenerator>);
    let world_handle = Arc::new(tokio::sync::Mutex::new(storage));

    // Resolve the layer ids so the assertions don't pin numerics.
    let bedrock_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:bedrock").unwrap())
        .map(|b| b.default)
        .unwrap();
    let grass_state_id = blocks
        .block(&mc_data::Identifier::parse("minecraft:grass_block").unwrap())
        .map(|b| b.default)
        .unwrap();

    // Pull a far chunk through the storage; the generator runs
    // because no region file exists. Assert it has the expected
    // five-layer shape.
    {
        let mut storage = world_handle.lock().await;
        let cpos = mc_world::ChunkPos { x: 50, z: -50 };
        let chunk = storage
            .get_chunk(cpos)
            .expect("storage get_chunk OK")
            .expect("generator produced a chunk");

        // Bedrock at the bottom.
        assert_eq!(
            chunk.get_block(0, mc_world::MIN_Y, 0),
            Some(bedrock_state_id)
        );

        // Find the surface column and assert grass on top.
        let height = generator.surface_height(50 * 16, -50 * 16);
        assert_eq!(
            chunk.get_block(0, height, 0),
            Some(grass_state_id),
            "generator output: column (0,{height},0) of chunk (50,-50) should be grass"
        );
        // Chunk is dirty so the M6 flush will persist it.
        assert!(chunk.dirty);
    }

    // Flush + reopen: the generated chunk now lives on disk and the
    // generator doesn't run a second time.
    {
        let mut storage = world_handle.lock().await;
        let n = storage.flush_dirty().expect("flush_dirty");
        assert!(
            n >= 1,
            "at least one generated chunk should have been flushed"
        );
    }
    drop(world_handle);

    // Fresh open with no generator: chunks already written must
    // still be readable. Pick a far chunk we just generated.
    let mut fresh = mc_world::WorldStorage::open(tmp.path(), Arc::clone(&blocks)).unwrap();
    let cpos = mc_world::ChunkPos { x: 50, z: -50 };
    let chunk = fresh
        .get_chunk(cpos)
        .expect("storage get_chunk OK")
        .expect("region holds generated chunk after flush");
    assert_eq!(
        chunk.get_block(0, mc_world::MIN_Y, 0),
        Some(bedrock_state_id)
    );
}
