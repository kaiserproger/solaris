use super::{
    BlockEdit, BlockStateId, Chunk, ChunkPos, Identifier, button_test_registry,
    in_memory_button_world, plan_toggle_block_interaction,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
#[test]
fn button_press_schedules_release_tick_without_global_scan() {
    let blocks = Arc::new(button_test_registry());
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    world
        .set_block_at(pos, mc_world::BlockStateId(1))
        .expect("place unpowered button");

    let plan = plan_toggle_block_interaction(&blocks, &world, pos, mc_world::BlockStateId(1), 100)
        .expect("button press should be interactive");

    assert_eq!(
        plan.edits,
        vec![BlockEdit {
            pos,
            new_state: mc_world::BlockStateId(2)
        }]
    );
    assert_eq!(plan.preconditions.len(), 1);
    assert_eq!(plan.preconditions[0].pos, pos);
    assert_eq!(
        plan.preconditions[0].expected_state,
        mc_world::BlockStateId(1)
    );
    assert_eq!(plan.scheduled_block_ticks.len(), 1);
    assert_eq!(plan.scheduled_block_ticks[0].pos, pos);
    assert_eq!(plan.scheduled_block_ticks[0].trigger_tick, 120);
    let ticks = world
        .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
        .expect("read scheduled block ticks")
        .expect("loaded chunk should expose ticks");
    assert!(ticks.is_empty(), "planning must not mutate world storage");
}

#[test]
fn button_press_does_not_materialize_unloaded_adjacent_chunks() {
    struct CountingAirGenerator {
        calls: Arc<AtomicUsize>,
    }

    impl mc_world::ChunkGenerator for CountingAirGenerator {
        fn generate(&self, pos: ChunkPos) -> Chunk {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let mut chunk = Chunk::empty(
                pos,
                BlockStateId(0),
                Identifier::parse("minecraft:plains").unwrap(),
            );
            chunk.status = "minecraft:full".into();
            chunk.dirty = true;
            chunk
        }
    }

    let blocks = Arc::new(button_test_registry());
    let generated_chunks = Arc::new(AtomicUsize::new(0));
    let mut world = in_memory_button_world(Arc::clone(&blocks)).with_generator(Arc::new(
        CountingAirGenerator {
            calls: Arc::clone(&generated_chunks),
        },
    ));
    let pos = mc_world::BlockPos { x: 15, y: 64, z: 1 };
    world
        .set_block_at(pos, mc_world::BlockStateId(1))
        .expect("place edge button");

    let plan = plan_toggle_block_interaction(&blocks, &world, pos, mc_world::BlockStateId(1), 100)
        .expect("edge button press should be interactive");

    assert_eq!(
        plan.edits,
        vec![BlockEdit {
            pos,
            new_state: mc_world::BlockStateId(2)
        }]
    );
    assert_eq!(generated_chunks.load(Ordering::Relaxed), 0);
    assert_eq!(world.cache_len(), 1);
}

#[test]
fn powered_button_press_is_consumed_without_duplicate_release_tick() {
    let blocks = Arc::new(button_test_registry());
    let mut world = in_memory_button_world(Arc::clone(&blocks));
    let pos = mc_world::BlockPos { x: 1, y: 64, z: 1 };
    world
        .set_block_at(pos, mc_world::BlockStateId(2))
        .expect("place powered button");
    world
        .schedule_block_tick(mc_world::ScheduledBlockTick::new(
            pos,
            Identifier::parse("minecraft:stone_button").unwrap(),
            120,
            0,
        ))
        .expect("schedule existing button release");

    let plan = plan_toggle_block_interaction(&blocks, &world, pos, mc_world::BlockStateId(2), 105)
        .expect("already powered button should still consume the interaction");

    assert!(plan.edits.is_empty());
    let ticks = world
        .scheduled_block_ticks(ChunkPos { x: 0, z: 0 })
        .expect("read scheduled block ticks")
        .expect("loaded chunk should expose ticks");
    assert_eq!(ticks.len(), 1);
    assert_eq!(ticks[0].trigger_tick, 120);
}
