use super::*;

#[test]
fn retained_clean_chunks_defer_try_publication_until_release() {
    for generated in [false, true] {
        let registry = Arc::new(BlockRegistry::from_report(&[]).unwrap());
        let mut world = WorldStorage::in_memory_with_capacity(registry, 1);
        let old = ChunkPos { x: 0, z: 0 };
        let next = ChunkPos { x: 1, z: 0 };
        let biome = Identifier::parse("minecraft:plains").unwrap();
        world
            .insert_generated_chunk(old, Chunk::empty(old, BlockStateId(0), biome.clone()))
            .unwrap();
        world.get_chunk_mut(old).unwrap().unwrap().dirty = false;
        let view = world.read_view();
        view.retain_chunk(old);

        let chunk = Chunk::empty(next, BlockStateId(0), biome.clone());
        let published = if generated {
            world.try_insert_generated_chunk(next, chunk)
        } else {
            world
                .try_commit_chunk_snapshot(next, chunk)
                .map(|chunk| chunk.is_some())
        };
        assert!(!published.expect("retained clean chunks must backpressure, not fail"));
        assert_eq!(world.cache_len(), 1);
        assert!(world.cached_chunk_snapshot(old).is_some());
        assert!(world.cached_chunk_snapshot(next).is_none());

        view.release_chunk(old);
        let chunk = Chunk::empty(next, BlockStateId(0), biome);
        let published = if generated {
            world.try_insert_generated_chunk(next, chunk)
        } else {
            world
                .try_commit_chunk_snapshot(next, chunk)
                .map(|chunk| chunk.is_some())
        };
        assert!(published.unwrap());
        assert_eq!(world.cache_len(), 1);
        assert!(world.cached_chunk_snapshot(old).is_none());
        assert!(world.cached_chunk_snapshot(next).is_some());
    }
}

#[test]
fn try_publication_preserves_non_pressure_errors() {
    for generated in [false, true] {
        let registry = Arc::new(BlockRegistry::from_report(&[]).unwrap());
        let mut world = WorldStorage::in_memory_with_capacity(registry, 1);
        let expected = ChunkPos { x: 1, z: 0 };
        let chunk = Chunk::empty(
            ChunkPos { x: 0, z: 0 },
            BlockStateId(0),
            Identifier::parse("minecraft:plains").unwrap(),
        );
        let result = if generated {
            world.try_insert_generated_chunk(expected, chunk)
        } else {
            world
                .try_commit_chunk_snapshot(expected, chunk)
                .map(|chunk| chunk.is_some())
        };
        assert!(matches!(
            result,
            Err(WorldError::ChunkPositionMismatch { .. })
        ));
        assert_eq!(world.cache_len(), 0);
    }
}

#[test]
fn disk_load_defers_unrequested_payload_corruption_without_hiding_requested_errors() {
    let registry = test_support::air_stone_registry();
    let temp = tempfile::tempdir().unwrap();
    let region_dir = temp.path().join("region");
    std::fs::create_dir_all(&region_dir).unwrap();
    let path = region_dir.join("r.0.0.mca");
    let mut payloads = Vec::new();
    for x in [0, 1] {
        let mut chunk = Chunk::empty(
            ChunkPos { x, z: 0 },
            BlockStateId(0),
            Identifier::parse("minecraft:plains").unwrap(),
        );
        chunk.set_block(0, 64, 0, BlockStateId(1));
        payloads.push(crate::anvil::chunk_to_payload(&chunk, &registry, 0).unwrap());
    }
    crate::anvil::write_region(&path, &payloads).unwrap();
    let mut bytes = std::fs::read(&path).unwrap();
    let location = u32::from_be_bytes(bytes[4..8].try_into().unwrap());
    let start = (location >> 8) as usize * 4096;
    bytes[start + 4] = 0x7f;
    std::fs::write(&path, bytes).unwrap();

    let mut world = WorldStorage::open_with_capacity(temp.path(), registry, 1).unwrap();
    let healthy = BlockPos { x: 0, y: 64, z: 0 };
    assert_eq!(world.get_block(healthy).unwrap(), Some(BlockStateId(1)));
    assert!(matches!(
        world.get_chunk(ChunkPos { x: 1, z: 0 }),
        Err(WorldError::Region(RegionError::UnknownCompression(0x7f)))
    ));
    assert_eq!(world.get_block(healthy).unwrap(), Some(BlockStateId(1)));
    assert!(
        world
            .cached_chunk_snapshot(ChunkPos { x: 1, z: 0 })
            .is_none()
    );
}
