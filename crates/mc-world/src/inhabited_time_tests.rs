use std::sync::Arc;

use mc_data::Identifier;
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_nbt::Tag;

use crate::{BlockRegistry, BlockStateId, Chunk, ChunkPos, WorldStorage};

fn chunk(position: ChunkPos) -> Chunk {
    Chunk::empty(
        position,
        BlockStateId(0),
        Identifier::parse("minecraft:plains").unwrap(),
    )
}

fn air_registry() -> Arc<BlockRegistry> {
    Arc::new(
        BlockRegistry::from_report(&[BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: Default::default(),
            states: vec![BlockStateReport {
                id: 0,
                default: true,
                properties: Default::default(),
            }],
        }])
        .unwrap(),
    )
}

#[test]
fn chunk_inhabited_time_accumulates_and_marks_dirty() {
    let position = ChunkPos { x: 2, z: -3 };
    let mut chunk = chunk(position);
    chunk.extras.push(("InhabitedTime".into(), Tag::Long(41)));
    chunk.dirty = false;

    chunk.increment_inhabited_time(7);

    assert_eq!(chunk.inhabited_time(), 48);
    assert!(chunk.dirty);
}

#[test]
fn zero_inhabited_time_delta_is_a_clean_noop() {
    let mut chunk = chunk(ChunkPos { x: 0, z: 0 });
    chunk.dirty = false;

    chunk.increment_inhabited_time(0);

    assert_eq!(chunk.inhabited_time(), 0);
    assert!(!chunk.dirty);
}

#[test]
fn resident_batch_updates_only_present_chunks() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).unwrap());
    let mut world = WorldStorage::in_memory(registry);
    let present = ChunkPos { x: 1, z: 1 };
    let missing_position = ChunkPos { x: 9, z: 9 };
    world
        .insert_generated_chunk(present, chunk(present))
        .unwrap();
    let mutation = world.mutation_view();

    let missing =
        mutation.increment_chunk_inhabited_times(&[(present, 20), (missing_position, 20)]);

    assert_eq!(missing, vec![(missing_position, 20)]);
    assert_eq!(world.cached_chunk(present).unwrap().inhabited_time(), 20);
}

#[test]
fn accumulated_inhabited_time_survives_anvil_flush_and_reopen() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("region")).unwrap();
    let registry = air_registry();
    let position = ChunkPos { x: -2, z: 5 };
    let mut world = WorldStorage::open(root.path(), Arc::clone(&registry)).unwrap();
    world
        .insert_generated_chunk(position, chunk(position))
        .unwrap();

    assert!(
        world
            .mutation_view()
            .increment_chunk_inhabited_times(&[(position, 37)])
            .is_empty()
    );
    assert_eq!(world.flush_dirty_at_tick(91).unwrap(), 1);
    drop(world);

    let mut reopened = WorldStorage::open(root.path(), registry).unwrap();
    assert_eq!(
        reopened
            .get_chunk(position)
            .unwrap()
            .unwrap()
            .inhabited_time(),
        37
    );
}
