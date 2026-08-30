use super::SessionRegistry;

use crate::operator_metrics::RetainedSaveReport;
use crate::server::{SaveAllReport, SaveAllTimings};

fn registry() -> SessionRegistry {
    SessionRegistry::default()
}

#[test]
fn entity_category_counts_cover_every_tracked_category_when_empty() {
    let counts = registry().entity_category_counts();
    let keys = counts.keys().map(String::as_str).collect::<Vec<_>>();
    assert_eq!(
        keys,
        vec![
            "area_effect_cloud",
            "ender_dragon",
            "evoker_fang",
            "hostile",
            "iron_golem",
            "natural_aquatic",
            "natural_ground",
            "natural_hostile",
            "sheep",
            "villager",
        ]
    );
    assert!(counts.values().all(|count| *count == 0));
}

#[test]
fn retained_save_report_roundtrips_latest_completion() {
    let sessions = registry();
    assert!(sessions.retained_save_report().is_none());

    let first = SaveAllReport {
        players_saved: 1,
        entities_saved: 2,
        chunks_flushed: 3,
        world_metadata_saved: true,
        timings: SaveAllTimings::default(),
        errors: Vec::new(),
    };
    sessions.retain_save_report(&first);
    let second = SaveAllReport {
        players_saved: 4,
        ..first.clone()
    };
    sessions.retain_save_report(&second);

    let RetainedSaveReport { report, .. } = sessions.retained_save_report().expect("retained");
    assert_eq!(report.players_saved, 4);
}

#[test]
fn retained_natural_spawn_report_roundtrips_latest_surface() {
    use mc_entity::natural_spawn_26_1_2::{NaturalSpawnCategoryReport, NaturalSpawnReport};

    let sessions = registry();
    assert!(sessions.retained_natural_spawn_report().is_none());

    let report = NaturalSpawnReport {
        hostile: NaturalSpawnCategoryReport {
            attempts: 7,
            committed: 2,
            ..NaturalSpawnCategoryReport::default()
        },
        ..NaturalSpawnReport::default()
    };
    sessions.retain_natural_spawn_report(1_200, report);

    let retained = sessions
        .retained_natural_spawn_report()
        .expect("retained natural spawn report");
    assert_eq!(retained.tick, 1_200);
    assert_eq!(retained.report.hostile.attempts, 7);
    assert_eq!(retained.report.hostile.committed, 2);
}

#[test]
fn online_player_names_are_empty_without_sessions() {
    let (names, truncated) = registry().online_player_names(16);
    assert_eq!(names, Vec::<String>::new());
    assert!(!truncated);
}
