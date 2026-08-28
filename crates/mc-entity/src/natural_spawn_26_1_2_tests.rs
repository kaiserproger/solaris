use std::collections::HashSet;
use std::sync::Arc;

use crate::Vec3;
use crate::natural_spawn_26_1_2::{
    HerdSpawn, MAX_HOSTILE_SPAWNS_PER_CHUNK, MAX_PASSIVE_SPAWNS_PER_CHUNK, NaturalSpawnCategory,
    NaturalSpawnCategoryReport, NaturalSpawnReport, NaturalSpawnScheduler,
    build_herd_spawn_candidates, choose_biome_spawn, herd_entry_count, hostile_chunk_spawns,
    passive_chunk_spawns, safe_land_spawn_offset, sheep_color_for_rolls,
    spawn_far_enough_from_players,
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
fn deterministic_spawn_randomness_lives_in_entity_domain() {
    use mc_data::biomes::{BiomeSpawnEntry, SheepColorClimate};

    assert!(passive_chunk_spawns((0, 0)));
    assert!(hostile_chunk_spawns((0, 0)));
    assert!((0..128).any(|x| !passive_chunk_spawns((x, 7))));
    assert!((0..128).any(|x| !hostile_chunk_spawns((x, 7))));

    assert_eq!(
        sheep_color_for_rolls(SheepColorClimate::Temperate, 0, 0),
        crate::SheepColor::Black
    );
    assert_eq!(
        sheep_color_for_rolls(SheepColorClimate::Warm, 99, 499),
        crate::SheepColor::Pink
    );
    assert_eq!(safe_land_spawn_offset(0), 0.48);
    assert_eq!(safe_land_spawn_offset(3), 0.51);

    let cow = mc_data::Identifier::parse("minecraft:cow").unwrap();
    let sheep = mc_data::Identifier::parse("minecraft:sheep").unwrap();
    let entries = vec![
        BiomeSpawnEntry {
            entity_type: cow,
            min_count: 2,
            max_count: 4,
            weight: 0,
        },
        BiomeSpawnEntry {
            entity_type: sheep.clone(),
            min_count: 3,
            max_count: 5,
            weight: 10,
        },
    ];
    assert_eq!(
        choose_biome_spawn(&entries, (11, -3), 2).map(|entry| &entry.entity_type),
        Some(&sheep)
    );
    let count = herd_entry_count(&entries[1], (11, -3), 2);
    assert!((3..=5).contains(&count));
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
