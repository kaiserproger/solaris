use std::collections::{HashMap, HashSet};

use mc_entity::EntityId;

use super::entity_simulation::split_goal_population_by_distance;

fn population(ids: &[i32]) -> HashSet<EntityId> {
    ids.iter().copied().map(EntityId).collect()
}

fn chunk_map(entries: &[(i32, (i32, i32))]) -> HashMap<EntityId, (i32, i32)> {
    entries
        .iter()
        .copied()
        .map(|(id, chunk)| (EntityId(id), chunk))
        .collect()
}

#[test]
fn near_entities_plan_every_tick_and_never_defer() {
    let population = population(&[1, 2, 3]);
    let chunks = chunk_map(&[(1, (0, 0)), (2, (1, 1)), (3, (-1, 0))]);
    let players = vec![(0, 0)];
    for tick in 100..104 {
        let (planned, deferred) = split_goal_population_by_distance(
            &population,
            |entity| chunks.get(&entity).copied(),
            &players,
            tick,
        );
        assert_eq!(planned, population);
        assert!(deferred.is_empty());
    }
}

#[test]
fn far_entities_plan_exactly_once_per_cadence_window() {
    let population = population(&[11, 12, 13, 14]);
    let chunks = chunk_map(&[(11, (5, 5)), (12, (0, 7)), (13, (-6, 2)), (14, (3, -4))]);
    let players = vec![(0, 0)];
    for entity in population.iter() {
        let planned_ticks = (100..104)
            .filter(|tick| {
                split_goal_population_by_distance(
                    &population,
                    |id| chunks.get(&id).copied(),
                    &players,
                    *tick,
                )
                .0
                .contains(entity)
            })
            .count();
        assert_eq!(planned_ticks, 1, "entity {entity:?} planned count");
    }
}

#[test]
fn split_covers_population_without_overlap() {
    let population = population(&[1, 11, 21]);
    let chunks = chunk_map(&[(1, (0, 0)), (11, (9, 9))]);
    let players = vec![(0, 0)];
    for tick in 200..208 {
        let (planned, deferred) = split_goal_population_by_distance(
            &population,
            |entity| chunks.get(&entity).copied(),
            &players,
            tick,
        );
        assert!(planned.is_disjoint(&deferred));
        let union: HashSet<EntityId> = planned.union(&deferred).copied().collect();
        assert_eq!(union, population);
    }
}

#[test]
fn unknown_chunk_entities_fail_open_to_planned() {
    let population = population(&[31]);
    let chunks = chunk_map(&[]);
    let players = vec![(0, 0)];
    for tick in 300..304 {
        let (planned, deferred) = split_goal_population_by_distance(
            &population,
            |entity| chunks.get(&entity).copied(),
            &players,
            tick,
        );
        assert_eq!(planned, population);
        assert!(deferred.is_empty());
    }
}

#[test]
fn empty_player_chunks_fail_open_to_planned() {
    let population = population(&[41]);
    let chunks = chunk_map(&[(41, (0, 0))]);
    let players = vec![];
    for tick in 400..404 {
        let (planned, deferred) = split_goal_population_by_distance(
            &population,
            |entity| chunks.get(&entity).copied(),
            &players,
            tick,
        );
        assert_eq!(planned, population);
        assert!(deferred.is_empty());
    }
}
