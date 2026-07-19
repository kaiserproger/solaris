use super::super::test_support::{air_stone_furnace_registry, air_stone_hopper_registry};
use super::*;
use crate::block::{BlockRegistry, BlockStateId};
use crate::chunk::{
    BlockPos, Chunk, ChunkGenerator, ChunkPos, FurnaceBlockEntity, HopperBlockEntity,
    ScheduledBlockTick, ScheduledFluidTick,
};
use mc_data::Identifier;

#[test]
fn chunk_source_view_tracks_generator_and_resident_chunks() {
    struct StubGenerator;

    impl ChunkGenerator for StubGenerator {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            Chunk::empty(
                pos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            )
        }
    }

    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let source = world.chunk_source_view();
    let position = ChunkPos { x: 2, z: -3 };
    assert_eq!(source.source_for(position), ChunkPrepareSource::Absent);

    world.set_generator(Some(Arc::new(StubGenerator)));
    assert_eq!(source.source_for(position), ChunkPrepareSource::Generator);

    world
        .insert_generated_chunk(position, StubGenerator.generate(position))
        .unwrap();
    assert_eq!(source.source_for(position), ChunkPrepareSource::Resident);
}

#[test]
fn chunk_source_view_recognizes_region_file() {
    let tmp = tempfile::tempdir().unwrap();
    let region = tmp.path().join("region");
    std::fs::create_dir_all(&region).unwrap();
    std::fs::write(region.join("r.0.0.mca"), []).unwrap();
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = WorldStorage::open(tmp.path(), registry).unwrap();

    assert_eq!(
        world
            .chunk_source_view()
            .source_for(ChunkPos { x: 17, z: 4 }),
        ChunkPrepareSource::RegionFile
    );
}

#[test]
fn read_view_publishes_immutable_chunk_edits() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let read_view = world.read_view();
    let cpos = ChunkPos { x: 0, z: 0 };
    let pos = BlockPos { x: 1, y: 0, z: 1 };
    world
        .insert_generated_chunk(
            cpos,
            Chunk::empty(
                cpos,
                BlockStateId(0),
                mc_data::Identifier::parse("minecraft:plains").unwrap(),
            ),
        )
        .unwrap();

    let before = read_view.snapshot_chunks(&[cpos]);
    assert_eq!(before.get_cached_block(pos), Some(BlockStateId(0)));
    assert_eq!(read_view.get_cached_block(pos), Some(BlockStateId(0)));
    let before_token = before.block_mutation_token(pos).unwrap();
    let view_token = read_view.block_mutation_token(pos).unwrap();

    world.set_block_at(pos, BlockStateId(1)).unwrap();

    let after = read_view.snapshot_chunks(&[cpos]);
    assert_eq!(after.get_cached_block(pos), Some(BlockStateId(1)));
    assert_eq!(read_view.get_cached_block(pos), Some(BlockStateId(1)));
    assert_eq!(before.get_cached_block(pos), Some(BlockStateId(0)));
    assert_eq!(before_token.version, 0);
    assert_eq!(view_token.version, 0);
    assert_eq!(after.block_mutation_token(pos).unwrap().version, 1);
    assert_eq!(read_view.block_mutation_token(pos).unwrap().version, 1);
}

#[test]
fn read_view_writer_does_not_block_an_independent_region() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let world = WorldStorage::in_memory(registry);
    let read_view = world.read_view();
    let held_region = ChunkPos { x: 0, z: 0 };
    let independent_region = ChunkPos { x: 8, z: 0 };
    let held = read_view.lock_chunk_shard_for_test(held_region);
    let worker_view = read_view.clone();
    let (completed, observed) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let block = worker_view.get_cached_block(BlockPos {
            x: independent_region.x * SECTION_DIM as i32,
            y: 0,
            z: independent_region.z * SECTION_DIM as i32,
        });
        completed.send(block).expect("reader completion");
    });

    let result = observed.recv_timeout(std::time::Duration::from_secs(1));
    drop(held);
    worker.join().expect("independent reader");

    assert_eq!(result, Ok(None));
}

#[test]
fn read_snapshot_returns_shared_chunk_by_position() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();

    let snapshot = world.read_view().snapshot_chunks(&[cpos]);
    let chunk = snapshot.chunk(cpos).expect("published chunk is present");

    assert_eq!(chunk.pos, cpos);
    assert!(Arc::ptr_eq(
        &chunk,
        &world.cached_chunk_snapshot(cpos).unwrap()
    ));
}

#[test]
fn read_view_removes_evicted_clean_chunks() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory_with_capacity(Arc::clone(&registry), 1);
    let read_view = world.read_view();
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    let first = ChunkPos { x: 0, z: 0 };
    let second = ChunkPos { x: 1, z: 0 };
    world
        .commit_chunk_snapshot(first, Chunk::empty(first, BlockStateId(0), biome.clone()))
        .unwrap();
    assert_eq!(
        read_view
            .snapshot_chunks(&[first])
            .get_cached_block(BlockPos { x: 0, y: 0, z: 0 }),
        Some(BlockStateId(0))
    );

    world
        .commit_chunk_snapshot(second, Chunk::empty(second, BlockStateId(0), biome))
        .unwrap();

    assert!(
        read_view
            .snapshot_chunks(&[first])
            .get_cached_block(BlockPos { x: 0, y: 0, z: 0 })
            .is_none()
    );
}

#[test]
fn cached_chunk_snapshots_are_shared_and_copy_on_write() {
    let air = mc_data::blocks::BlockReport {
        id: mc_data::Identifier::parse("minecraft:air").unwrap(),
        properties: std::collections::BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 0,
            default: true,
            properties: std::collections::BTreeMap::new(),
        }],
    };
    let stone = mc_data::blocks::BlockReport {
        id: mc_data::Identifier::parse("minecraft:stone").unwrap(),
        properties: std::collections::BTreeMap::new(),
        states: vec![mc_data::blocks::BlockStateReport {
            id: 1,
            default: true,
            properties: std::collections::BTreeMap::new(),
        }],
    };
    let registry = Arc::new(BlockRegistry::from_report(&[air, stone]).unwrap());
    let cpos = ChunkPos { x: 0, z: 0 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();

    let before = world.cached_chunk_snapshot(cpos).unwrap();
    assert_eq!(before.get_block(1, 0, 1), Some(BlockStateId(0)));

    assert_eq!(
        world
            .set_block_at(BlockPos { x: 1, y: 0, z: 1 }, BlockStateId(1))
            .unwrap(),
        Some(BlockStateId(0))
    );
    let after = world.cached_chunk_snapshot(cpos).unwrap();

    assert_eq!(before.get_block(1, 0, 1), Some(BlockStateId(0)));
    assert_eq!(after.get_block(1, 0, 1), Some(BlockStateId(1)));
    assert!(!Arc::ptr_eq(&before, &after));
}

#[test]
fn read_view_tracks_furnace_set_and_block_replacement() {
    let registry = air_stone_furnace_registry();
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let read_view = world.read_view();
    let cpos = ChunkPos { x: 0, z: 0 };
    let pos = BlockPos { x: 1, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    world.set_block_at(pos, BlockStateId(2)).unwrap();
    let furnace = FurnaceBlockEntity {
        burn_remaining: 10,
        burn_total: 10,
        ..FurnaceBlockEntity::default()
    };
    world
        .set_furnace_block_entity(pos, furnace.clone())
        .unwrap();

    assert_eq!(read_view.furnace_snapshots(&[cpos]), vec![(pos, furnace)]);

    world.set_block_at(pos, BlockStateId(1)).unwrap();

    assert!(read_view.furnace_snapshots(&[cpos]).is_empty());
}

#[test]
fn scheduled_tick_view_tracks_block_queue_changes() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    let pos = BlockPos { x: 1, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    let block = mc_data::Identifier::parse("minecraft:wheat").unwrap();
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    let scheduled_ticks = world.scheduled_tick_view();

    assert!(!scheduled_ticks.block_due(cpos, 20));
    assert!(
        world
            .schedule_block_tick(ScheduledBlockTick::new(pos, block, 20, 0))
            .unwrap()
    );
    assert!(!scheduled_ticks.block_due(cpos, 19));
    assert!(scheduled_ticks.block_due(cpos, 20));

    assert_eq!(
        world
            .drain_due_block_ticks(cpos, 20, usize::MAX)
            .unwrap()
            .len(),
        1
    );
    assert!(!scheduled_ticks.block_due(cpos, u64::MAX));
}

#[test]
fn scheduled_tick_view_flags_hopper_without_tick_for_backfill() {
    let registry = air_stone_hopper_registry();
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    let pos = BlockPos { x: 1, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    let scheduled_ticks = world.scheduled_tick_view();

    world.set_block_at(pos, BlockStateId(2)).unwrap();
    world
        .set_hopper_block_entity(pos, HopperBlockEntity::default())
        .unwrap();

    assert!(scheduled_ticks.block_due(cpos, 0));
}

#[test]
fn scheduled_tick_view_tracks_fluid_queue_changes() {
    let registry = Arc::new(BlockRegistry::from_report(&[]).expect("empty registry builds"));
    let mut world = WorldStorage::in_memory(Arc::clone(&registry));
    let cpos = ChunkPos { x: 0, z: 0 };
    let pos = BlockPos { x: 1, y: 2, z: 3 };
    let biome = mc_data::Identifier::parse("minecraft:plains").unwrap();
    let fluid = mc_data::Identifier::parse("minecraft:water").unwrap();
    world
        .insert_generated_chunk(cpos, Chunk::empty(cpos, BlockStateId(0), biome))
        .unwrap();
    let scheduled_ticks = world.scheduled_tick_view();

    assert!(
        world
            .schedule_fluid_tick(ScheduledFluidTick::new(pos, fluid, 30, 0))
            .unwrap()
    );
    assert!(!scheduled_ticks.fluid_due(cpos, 29));
    assert!(scheduled_ticks.fluid_due(cpos, 30));

    assert_eq!(world.remove_scheduled_fluid_ticks_at(pos).unwrap().len(), 1);
    assert!(!scheduled_ticks.fluid_due(cpos, u64::MAX));
}
