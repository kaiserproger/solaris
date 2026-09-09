use std::sync::Arc;

use mc_data::Identifier;

use super::*;
use crate::block::BlockStateId;
use crate::chunk::{BlockPos, Chunk, ChunkPos};
use crate::storage::test_support::single_air_registry;

fn empty_chunk(pos: ChunkPos) -> Chunk {
    Chunk::empty(
        pos,
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    )
}

#[test]
fn all_dirty_cache_stops_at_hard_count_cap() {
    let registry = single_air_registry();
    let mut world = WorldStorage::in_memory_with_capacity(registry, 2);
    world
        .set_chunk_byte_budgets(64 * 1024 * 1024, 48 * 1024 * 1024)
        .unwrap();

    for x in 0..2 {
        world
            .insert_generated_chunk(ChunkPos { x, z: 0 }, empty_chunk(ChunkPos { x, z: 0 }))
            .unwrap();
    }
    let error = world
        .insert_generated_chunk(
            ChunkPos { x: 2, z: 0 },
            empty_chunk(ChunkPos { x: 2, z: 0 }),
        )
        .unwrap_err();

    assert!(matches!(error, WorldError::ChunkCachePressure { .. }));
    assert_eq!(world.cache_len(), 2);
    assert_eq!(world.dirty_count(), 2);
}

#[test]
fn resident_and_dirty_byte_budgets_reject_large_chunks() {
    let registry = single_air_registry();
    let mut world = WorldStorage::in_memory_with_capacity(registry, 8);
    let first = empty_chunk(ChunkPos { x: 0, z: 0 });
    let first_bytes = first.estimated_heap_bytes();
    world
        .set_chunk_byte_budgets(first_bytes + 4096, first_bytes + 4096)
        .unwrap();
    world
        .insert_generated_chunk(ChunkPos { x: 0, z: 0 }, first)
        .unwrap();

    let mut oversized = empty_chunk(ChunkPos { x: 1, z: 0 });
    oversized
        .block_entities
        .insert(BlockPos { x: 16, y: 0, z: 0 }, vec![0_u8; 8192]);
    let requested = oversized.estimated_heap_bytes();
    let error = world
        .insert_generated_chunk(ChunkPos { x: 1, z: 0 }, oversized)
        .unwrap_err();

    assert!(matches!(
        error,
        WorldError::ChunkCachePressure {
            requested_bytes,
            resident_budget,
            dirty_budget,
            ..
        } if requested_bytes == requested
            && resident_budget == first_bytes + 4096
            && dirty_budget == first_bytes + 4096
    ));
    let stats = world.stats();
    assert!(stats.resident_bytes <= stats.resident_byte_budget);
    assert!(stats.dirty_bytes <= stats.dirty_byte_budget);
}

#[test]
fn save_unhealthy_state_rejects_new_admission_until_recovered() {
    let registry = single_air_registry();
    let mut world = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 4);
    world
        .insert_generated_chunk(
            ChunkPos { x: 0, z: 0 },
            empty_chunk(ChunkPos { x: 0, z: 0 }),
        )
        .unwrap();
    world.mark_save_unhealthy();

    assert!(matches!(
        world.insert_generated_chunk(
            ChunkPos { x: 1, z: 0 },
            empty_chunk(ChunkPos { x: 1, z: 0 })
        ),
        Err(WorldError::ChunkCachePressure {
            save_healthy: false,
            ..
        })
    ));

    world.mark_save_healthy();
    world
        .insert_generated_chunk(
            ChunkPos { x: 1, z: 0 },
            empty_chunk(ChunkPos { x: 1, z: 0 }),
        )
        .unwrap();
    assert_eq!(world.cache_len(), 2);
}

#[test]
fn clean_eviction_recovers_reserved_capacity() {
    let registry = single_air_registry();
    let mut world = WorldStorage::in_memory_with_capacity(registry, 1);
    world
        .commit_chunk_snapshot(
            ChunkPos { x: 0, z: 0 },
            empty_chunk(ChunkPos { x: 0, z: 0 }),
        )
        .unwrap();
    world
        .commit_chunk_snapshot(
            ChunkPos { x: 1, z: 0 },
            empty_chunk(ChunkPos { x: 1, z: 0 }),
        )
        .unwrap();

    assert_eq!(world.cache_len(), 1);
    assert!(
        world
            .cached_chunk_snapshot(ChunkPos { x: 0, z: 0 })
            .is_none()
    );
    assert!(
        world
            .cached_chunk_snapshot(ChunkPos { x: 1, z: 0 })
            .is_some()
    );
}

#[test]
fn byte_admission_tracks_mutation_flush_and_eviction() {
    let position = ChunkPos { x: 0, z: 0 };
    let next_position = ChunkPos { x: 1, z: 0 };
    let first = empty_chunk(position);
    let chunk_bytes = first.estimated_heap_bytes();
    let mut world = WorldStorage::in_memory_with_capacity(single_air_registry(), 8);
    world
        .set_chunk_byte_budgets(chunk_bytes * 8, chunk_bytes * 2)
        .unwrap();
    world.commit_chunk_snapshot(position, first).unwrap();
    world
        .resident
        .mutate(position, |chunk| {
            chunk
                .block_entities
                .insert(BlockPos { x: 0, y: 0, z: 0 }, vec![0; chunk_bytes * 4]);
            chunk.mark_dirty();
        })
        .unwrap();
    assert!(matches!(
        world.insert_generated_chunk(next_position, empty_chunk(next_position)),
        Err(WorldError::ChunkCachePressure { .. })
    ));
    assert!(world.cached_chunk_snapshot(next_position).is_none());

    world
        .resident
        .mutate(position, |chunk| {
            chunk.block_entities.clear();
            chunk.block_entities.shrink_to_fit();
        })
        .unwrap();
    world
        .insert_generated_chunk(next_position, empty_chunk(next_position))
        .unwrap();
    assert!(world.cached_chunk_snapshot(next_position).is_some());
    assert_eq!(world.stats().dirty_bytes, chunk_bytes * 2);

    let flushed = world.cached_chunk_snapshot(position).unwrap();
    assert_eq!(
        world
            .resident
            .finalize_region_flush(vec![(position, flushed.dirty_generation, flushed,)]),
        1
    );
    let third_position = ChunkPos { x: 2, z: 0 };
    world
        .insert_generated_chunk(third_position, empty_chunk(third_position))
        .unwrap();
    assert!(world.cached_chunk_snapshot(third_position).is_some());
    assert_eq!(world.stats().dirty_bytes, chunk_bytes * 2);

    world
        .set_chunk_byte_budgets(chunk_bytes * 3, chunk_bytes * 2)
        .unwrap();
    let fourth_position = ChunkPos { x: 3, z: 0 };
    world
        .commit_chunk_snapshot(fourth_position, empty_chunk(fourth_position))
        .unwrap();
    assert!(world.cached_chunk_snapshot(position).is_none());
    assert!(world.cached_chunk_snapshot(fourth_position).is_some());
    assert_eq!(world.stats().resident_bytes, chunk_bytes * 3);
}
