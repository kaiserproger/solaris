use std::collections::HashSet;
use std::sync::Arc;

use crate::Vec3;
use crate::natural_spawn_26_1_2::{
    HerdSpawn, MAX_HOSTILE_SPAWNS_PER_CHUNK, MAX_PASSIVE_SPAWNS_PER_CHUNK, NaturalSpawnCategory,
    NaturalSpawnCategoryReport, NaturalSpawnReport, NaturalSpawnScheduler,
    build_herd_spawn_candidates, spawn_far_enough_from_players,
};

#[test]
fn scheduler_rotates_bounded_chunks_with_independent_category_cursors() {
    let active = Arc::new((0..6).map(|x| (x, 0)).collect::<HashSet<_>>());
    let mut scheduler = NaturalSpawnScheduler::default();

    assert_eq!(
        scheduler.select_chunks(NaturalSpawnCategory::Friendly, &active),
        vec![(0, 0), (1, 0), (2, 0), (3, 0)]
    );
    assert_eq!(
        scheduler.select_chunks(NaturalSpawnCategory::Friendly, &active),
        vec![(4, 0), (5, 0), (0, 0), (1, 0)]
    );
    assert_eq!(
        scheduler.select_chunks(NaturalSpawnCategory::Hostile, &active),
        vec![(0, 0), (1, 0), (2, 0), (3, 0)]
    );
}

#[test]
fn scheduler_reports_cumulative_metrics_only_at_the_bounded_log_interval() {
    let mut scheduler = NaturalSpawnScheduler::default();
    let first = NaturalSpawnReport {
        friendly: NaturalSpawnCategoryReport {
            attempts: 1,
            committed: 2,
            ..NaturalSpawnCategoryReport::default()
        },
        ..NaturalSpawnReport::default()
    };

    assert_eq!(scheduler.record(1_199, first), None);
    assert_eq!(
        scheduler.record(
            1_200,
            NaturalSpawnReport {
                hostile: NaturalSpawnCategoryReport {
                    attempts: 1,
                    rejected_darkness: 3,
                    ..NaturalSpawnCategoryReport::default()
                },
                ..NaturalSpawnReport::default()
            }
        ),
        Some(NaturalSpawnReport {
            friendly: NaturalSpawnCategoryReport {
                attempts: 1,
                committed: 2,
                ..NaturalSpawnCategoryReport::default()
            },
            hostile: NaturalSpawnCategoryReport {
                attempts: 1,
                rejected_darkness: 3,
                ..NaturalSpawnCategoryReport::default()
            },
        })
    );
}

#[test]
fn candidate_planning_keeps_distance_and_per_chunk_caps_in_entity_domain() {
    let player = Vec3::new(0.0, 64.0, 0.0);
    assert!(!spawn_far_enough_from_players(
        &[player],
        Vec3::new(24.0, 64.0, 0.0),
        24.0,
    ));
    assert!(spawn_far_enough_from_players(
        &[player],
        Vec3::new(24.5, 64.0, 0.0),
        24.0,
    ));

    let chunk = (2, 0);
    let behaviors = mc_data::mob_behavior_26_1_2::MobBehaviorTable::vanilla_26_1_2();
    let passive = (0..=MAX_PASSIVE_SPAWNS_PER_CHUNK)
        .map(|slot| HerdSpawn {
            chunk,
            slot: slot as u8,
            entity_type_id: 11,
            entity_type_name: "minecraft:cow".to_owned(),
            position: Vec3::new(40.5 + slot as f64, 65.0, 0.5),
            hostile: false,
            sheep_color: None,
        })
        .collect::<Vec<_>>();
    let hostile = (0..=MAX_HOSTILE_SPAWNS_PER_CHUNK)
        .map(|slot| HerdSpawn {
            chunk,
            slot: slot as u8,
            entity_type_id: 54,
            entity_type_name: "minecraft:zombie".to_owned(),
            position: Vec3::new(40.5 + slot as f64, 65.0, 0.5),
            hostile: true,
            sheep_color: None,
        })
        .collect::<Vec<_>>();

    assert_eq!(
        build_herd_spawn_candidates(chunk, &passive, &[player], 73, 24.0, &behaviors).len(),
        MAX_PASSIVE_SPAWNS_PER_CHUNK
    );
    assert_eq!(
        build_herd_spawn_candidates(chunk, &hostile, &[player], 73, 24.0, &behaviors).len(),
        MAX_HOSTILE_SPAWNS_PER_CHUNK
    );
}
