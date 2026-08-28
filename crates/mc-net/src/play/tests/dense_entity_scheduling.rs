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
