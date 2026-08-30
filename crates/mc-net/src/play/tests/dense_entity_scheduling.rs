use super::*;

#[test]
fn dense_entity_movement_tracking_rotates_bounded_shards() {
    let entity_count = ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN * 10;
    let mut visits = vec![0; entity_count];

    for turn in 0..10 {
        let tick = turn * ENTITY_MOVE_SEND_INTERVAL_TICKS;
        let mut due = 0;
        for (ordinal, visits) in visits.iter_mut().enumerate() {
            if ordinary_entity_is_due_for_movement_tracking(
                ordinal,
                tick,
                entity_count,
                ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN,
            ) {
                *visits += 1;
                due += 1;
            }
        }
        assert_eq!(due, ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN);
    }
    assert!(visits.into_iter().all(|visits| visits == 1));
}

#[test]
fn movement_tracking_uses_the_runtime_publication_budget_without_gaps() {
    let publication_budget = ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN * 2;
    let entity_count = publication_budget * 4;
    let mut visits = vec![0; entity_count];

    for turn in 0..4 {
        let tick = turn * ENTITY_MOVE_SEND_INTERVAL_TICKS;
        let mut due = 0;
        for (ordinal, visits) in visits.iter_mut().enumerate() {
            if ordinary_entity_is_due_for_movement_tracking(
                ordinal,
                tick,
                entity_count,
                publication_budget,
            ) {
                *visits += 1;
                due += 1;
            }
        }
        assert_eq!(due, publication_budget);
    }
    assert!(visits.into_iter().all(|visits| visits == 1));
}

#[test]
fn dense_natural_movement_tracking_rotates_every_tick() {
    let entity_count = ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN * 10;
    let entities = (0..entity_count)
        .map(|id| EntityId(i32::try_from(id).unwrap()))
        .collect::<HashSet<_>>();
    let mut visits = vec![0; entity_count];

    for tick in 0..10 {
        let due = bounded_entity_ids_due_for_tick(
            &entities,
            tick,
            ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN,
        );
        assert_eq!(due.len(), ENTITY_MOVEMENT_TARGET_UPDATES_PER_TRACKING_TURN);
        for entity in due {
            visits[usize::try_from(entity.0).unwrap()] += 1;
        }
    }
    assert!(visits.into_iter().all(|visits| visits == 1));
}

#[test]
fn dense_entity_goal_updates_rotate_bounded_cohorts() {
    let entity_count = ENTITY_GOAL_UPDATES_PER_TICK * 10;
    let entities = (0..entity_count)
        .map(|id| EntityId(i32::try_from(id).unwrap()))
        .collect::<HashSet<_>>();
    let mut visits = vec![0; entity_count];

    for tick in 0..10 {
        let due = entity_goal_ids_due_for_tick(&entities, tick, true);
        assert_eq!(due.len(), ENTITY_GOAL_UPDATES_PER_TICK);
        for entity in due {
            visits[usize::try_from(entity.0).unwrap()] += 1;
        }
    }
    assert!(visits.into_iter().all(|visits| visits == 1));
}

#[test]
fn dense_entity_simulation_cohorts_are_stratified_across_regions() {
    const REGION_COUNT: usize = 16;
    const ENTITIES_PER_REGION: usize = 2_500;
    const LIMIT: usize = 1_000;
    let entities = (0..REGION_COUNT * ENTITIES_PER_REGION)
        .map(|id| EntityId(i32::try_from(id).unwrap()))
        .collect::<HashSet<_>>();

    let due = bounded_entity_ids_due_for_tick(&entities, 17, LIMIT);
    let mut per_region = [0usize; REGION_COUNT];
    for entity in due {
        let region = usize::try_from(entity.0).unwrap() / ENTITIES_PER_REGION;
        per_region[region] += 1;
    }

    assert_eq!(per_region.iter().sum::<usize>(), LIMIT);
    assert!(per_region.iter().all(|count| (62..=63).contains(count)));
}

#[test]
fn ordinary_entity_goal_updates_keep_full_tick_cadence() {
    let entity_count = ENTITY_GOAL_UPDATES_PER_TICK + 88;
    let entities = (0..entity_count)
        .map(|id| EntityId(i32::try_from(id).unwrap()))
        .collect::<HashSet<_>>();

    assert_eq!(entity_goal_ids_due_for_tick(&entities, 7, false), entities);
}

/// The pre-radix implementation, kept verbatim as the semantic oracle: a full
/// comparison sort followed by stratified picking over sorted entity ids.
fn sorted_strata_reference(
    eligible_ids: &HashSet<EntityId>,
    tick: u64,
    limit: usize,
) -> HashSet<EntityId> {
    let limit = limit.max(1);
    if eligible_ids.len() <= limit {
        return eligible_ids.clone();
    }
    let mut ordered = eligible_ids.iter().copied().collect::<Vec<_>>();
    ordered.sort_unstable();
    let population = ordered.len();
    (0..limit)
        .map(|stratum| {
            let start = stratum.saturating_mul(population) / limit;
            let end = (stratum + 1).saturating_mul(population) / limit;
            let width = end.saturating_sub(start).max(1);
            ordered[start + tick as usize % width]
        })
        .collect()
}

fn xorshift64(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

/// Deterministic ids spanning the full `i32` range: sparse, negative, huge.
fn sparse_entity_ids(count: usize, seed: u64) -> HashSet<EntityId> {
    let mut state = seed | 1;
    let mut ids = HashSet::with_capacity(count);
    while ids.len() < count {
        ids.insert(EntityId(xorshift64(&mut state) as i32));
    }
    ids
}

#[test]
fn bounded_selection_returns_every_eligible_id_when_population_fits_limit() {
    let entities = sparse_entity_ids(40, 0x5EED);
    assert_eq!(bounded_entity_ids_due_for_tick(&entities, 5, 64), entities);
    assert_eq!(bounded_entity_ids_due_for_tick(&entities, 5, 40), entities);
}

#[test]
fn dense_entity_selection_matches_sorted_strata_reference_across_id_shapes() {
    let cases = [
        (257usize, 64usize),
        (1_000, 40),
        (1_001, 40),
        (40_000, 1_000),
        (513, 512),
        (2_048, 999),
    ];
    for (population, limit) in cases {
        let entities = sparse_entity_ids(population, 0x5EED_600D ^ population as u64);
        assert!(entities.iter().any(|id| id.0 < 0));
        assert!(entities.iter().any(|id| id.0 > u16::MAX as i32));
        for tick in [0u64, 1, 7, 39, 40, 999] {
            let due = bounded_entity_ids_due_for_tick(&entities, tick, limit);
            let reference = sorted_strata_reference(&entities, tick, limit);
            assert_eq!(
                due, reference,
                "population={population} limit={limit} tick={tick}"
            );
            assert_eq!(due.len(), limit);
            assert!(due.iter().all(|id| entities.contains(id)));
        }
    }
}

#[test]
fn dense_entity_selection_is_independent_of_insertion_order_and_hash_seed() {
    let population = 4_096;
    let limit = 103;
    let forward = sparse_entity_ids(population, 0xC0FFEE);
    let forward_order = forward.iter().copied().collect::<Vec<_>>();
    let mut reversed_set = HashSet::with_capacity(population);
    for id in forward_order.iter().rev() {
        reversed_set.insert(*id);
    }
    let mut ascending = forward.iter().copied().collect::<Vec<_>>();
    ascending.sort_unstable();
    let mut ascending_set = HashSet::with_capacity(population);
    for id in ascending {
        ascending_set.insert(id);
    }

    for tick in 0..40u64 {
        let expected = bounded_entity_ids_due_for_tick(&forward, tick, limit);
        assert_eq!(
            bounded_entity_ids_due_for_tick(&reversed_set, tick, limit),
            expected
        );
        assert_eq!(
            bounded_entity_ids_due_for_tick(&ascending_set, tick, limit),
            expected
        );
    }
}

#[test]
fn sparse_negative_ids_rotate_through_the_freshness_budget() {
    use std::collections::HashMap;

    // limit = ceil(N / 40): every stratum is exactly 40 wide, so 40 ticks must
    // hand every eligible entity exactly one turn.
    let population: usize = 8_000;
    let limit = population.div_ceil(40);
    let entities = sparse_entity_ids(population, 0xABCD_1234);
    let ids = entities.iter().copied().collect::<Vec<_>>();
    let indexes = ids
        .iter()
        .enumerate()
        .map(|(index, id)| (*id, index))
        .collect::<HashMap<_, _>>();
    let mut visits = vec![0usize; ids.len()];

    for tick in 0..40u64 {
        let due = bounded_entity_ids_due_for_tick(&entities, tick, limit);
        assert_eq!(due.len(), limit);
        for id in due {
            visits[indexes[&id]] += 1;
        }
    }

    assert!(visits.iter().all(|visits| *visits >= 1));
    assert_eq!(visits.iter().sum::<usize>(), 40 * limit);
    assert!(visits.into_iter().all(|visits| visits == 1));
}

#[test]
fn crafted_extreme_ids_rotate_one_per_tick() {
    let crafted = [i32::MIN, -123_456, -1, 0, 1, 720_000, 1 << 30, i32::MAX]
        .into_iter()
        .map(EntityId)
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();

    for tick in 0..8u64 {
        let due = bounded_entity_ids_due_for_tick(&crafted, tick, 1);
        assert_eq!(due.len(), 1);
        assert_eq!(due, sorted_strata_reference(&crafted, tick, 1));
        seen.extend(due);
    }
    assert_eq!(seen, crafted);
}

#[test]
#[ignore = "manual benchmark: cargo test -p mc-net --lib dense_entity -- --ignored --nocapture"]
fn dense_entity_selection_benchmark_helper() {
    use std::hint::black_box;
    use std::time::Instant;

    fn measure(
        label: &str,
        entities: &HashSet<EntityId>,
        limit: usize,
        iterations: u32,
        select: fn(&HashSet<EntityId>, u64, usize) -> HashSet<EntityId>,
    ) -> f64 {
        black_box(select(entities, 0, limit));
        let mut best = f64::INFINITY;
        for _ in 0..5 {
            let start = Instant::now();
            for tick in 0..u64::from(iterations) {
                black_box(select(entities, tick, limit));
            }
            let per_selection = start.elapsed().as_nanos() as f64 / f64::from(iterations);
            best = best.min(per_selection);
        }
        println!("  {label}: {best:.0} ns/selection");
        best
    }

    for (population, limit) in [(1_000usize, 25usize), (10_000, 250), (40_000, 1_000)] {
        let entities = (0..population)
            .map(|id| EntityId(id as i32))
            .collect::<HashSet<_>>();
        assert_eq!(
            bounded_entity_ids_due_for_tick(&entities, 7, limit),
            sorted_strata_reference(&entities, 7, limit)
        );
        println!("N={population} limit={limit} (contiguous ids)");
        let legacy = measure(
            "legacy sort_unstable",
            &entities,
            limit,
            200,
            sorted_strata_reference,
        );
        let radix = measure(
            "radix              ",
            &entities,
            limit,
            200,
            bounded_entity_ids_due_for_tick,
        );
        println!("  speedup: {:.2}x", legacy / radix);
    }

    let sparse = sparse_entity_ids(40_000, 0x0BAD_C0DE);
    assert_eq!(
        bounded_entity_ids_due_for_tick(&sparse, 7, 1_000),
        sorted_strata_reference(&sparse, 7, 1_000)
    );
    println!("N=40000 limit=1000 (sparse full-range ids)");
    let legacy = measure(
        "legacy sort_unstable",
        &sparse,
        1_000,
        200,
        sorted_strata_reference,
    );
    let radix = measure(
        "radix              ",
        &sparse,
        1_000,
        200,
        bounded_entity_ids_due_for_tick,
    );
    println!("  speedup: {:.2}x", legacy / radix);
}
