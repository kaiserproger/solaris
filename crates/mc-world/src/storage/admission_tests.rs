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
