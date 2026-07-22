use super::inhabited_time::InhabitedTimeAccumulator;
use mc_world::ChunkPos;

#[test]
fn active_chunks_accumulate_exact_ticks_before_batched_flush() {
    let mut accumulator = InhabitedTimeAccumulator::default();
    let shared = (0, 0);
    let short_lived = (1, -1);

    for tick in 1..20 {
        let active = if tick <= 7 {
            vec![shared, short_lived]
        } else {
            vec![shared]
        };
        let updates = accumulator.observe_tick(tick, &active);
        if tick == 8 {
            assert_eq!(updates, vec![(ChunkPos { x: 1, z: -1 }, 7)]);
        } else {
            assert!(updates.is_empty());
        }
    }
    let updates = accumulator.observe_tick(20, &[shared]);

    assert_eq!(updates, vec![(ChunkPos { x: 0, z: 0 }, 20)]);
}

#[test]
fn shutdown_drain_preserves_partial_interval() {
    let mut accumulator = InhabitedTimeAccumulator::default();
    assert!(accumulator.observe_tick(1, &[(3, 4)]).is_empty());
    assert!(accumulator.observe_tick(2, &[(3, 4)]).is_empty());

    assert_eq!(accumulator.drain(), vec![(ChunkPos { x: 3, z: 4 }, 2)]);
    assert!(accumulator.drain().is_empty());
}

#[test]
fn inactive_chunk_flushes_immediately_and_missing_update_can_be_restored() {
    let mut accumulator = InhabitedTimeAccumulator::default();
    assert!(accumulator.observe_tick(1, &[(2, 3)]).is_empty());
    assert!(accumulator.observe_tick(2, &[(2, 3)]).is_empty());

    let inactive = accumulator.observe_tick(3, &[]);
    assert_eq!(inactive, vec![(ChunkPos { x: 2, z: 3 }, 2)]);
    accumulator.restore(inactive);

    assert!(accumulator.observe_tick(4, &[]).is_empty());
    assert_eq!(accumulator.drain(), vec![(ChunkPos { x: 2, z: 3 }, 2)]);
}
