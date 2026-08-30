//! Operator-facing session facts for the optional first-party dashboard.
//!
//! Reads take the authoritative registry mutex briefly at dashboard poll rate
//! (never on the simulation hot path); retained reports are written only when
//! the owning authority already surfaces them.

use std::collections::BTreeMap;

use super::SessionRegistry;
use crate::operator_metrics::{RetainedNaturalSpawnReport, RetainedSaveReport};
use crate::server::SaveAllReport;
use mc_entity::natural_spawn_26_1_2::NaturalSpawnReport;
use std::time::Instant;

impl SessionRegistry {
    /// Retain the latest completed save report for operator reads.
    pub(crate) fn retain_save_report(&self, report: &SaveAllReport) {
        let mut retained = self
            .last_save_report
            .lock()
            .expect("save report lock poisoned");
        *retained = Some(RetainedSaveReport {
            at: Instant::now(),
            report: report.clone(),
        });
    }

    /// Retain the latest cumulative natural-spawn report for operator reads.
    pub(crate) fn retain_natural_spawn_report(&self, tick: u64, report: NaturalSpawnReport) {
        let mut retained = self
            .natural_spawn_report
            .lock()
            .expect("natural spawn report lock poisoned");
        *retained = Some(RetainedNaturalSpawnReport {
            at: Instant::now(),
            tick,
            report,
        });
    }

    /// Latest retained save report, if one completed.
    pub(crate) fn retained_save_report(&self) -> Option<RetainedSaveReport> {
        self.last_save_report
            .lock()
            .expect("save report lock poisoned")
            .clone()
    }

    /// Latest retained cumulative natural-spawn report, if one surfaced.
    pub(crate) fn retained_natural_spawn_report(&self) -> Option<RetainedNaturalSpawnReport> {
        *self
            .natural_spawn_report
            .lock()
            .expect("natural spawn report lock poisoned")
    }

    /// Bounded point-in-time online player names, sorted by session id.
    /// Returns the names and whether the limit truncated the view.
    pub(crate) fn online_player_names(&self, limit: usize) -> (Vec<String>, bool) {
        let inner = self.lock_inner("snapshot online player names for operator");
        let mut sessions = inner
            .sessions
            .iter()
            .filter(|(_, session)| !session.tx.is_closed())
            .collect::<Vec<_>>();
        sessions.sort_unstable_by_key(|(session_id, _)| **session_id);
        let truncated = sessions.len() > limit;
        (
            sessions
                .into_iter()
                .take(limit)
                .map(|(_, session)| session.name.clone())
                .collect(),
            truncated,
        )
    }

    /// Server entity counts by tracked category. Categories mirror the
    /// registry's maintained sets; entities outside every tracked set are only
    /// covered by the total entity counter in the runtime telemetry snapshot.
    pub(crate) fn entity_category_counts(&self) -> BTreeMap<String, u64> {
        let inner = self.lock_inner("snapshot entity categories for operator");
        [
            ("hostile", inner.hostile_entities.len()),
            ("natural_hostile", inner.natural_hostile_mobs.len()),
            ("natural_ground", inner.natural_ground_mobs.len()),
            ("natural_aquatic", inner.natural_aquatic_mobs.len()),
            ("sheep", inner.sheep_entities.len()),
            ("villager", inner.villager_entities.len()),
            ("iron_golem", inner.iron_golem_entities.len()),
            ("ender_dragon", inner.ender_dragon_entities.len()),
            ("area_effect_cloud", inner.area_effect_cloud_entities.len()),
            ("evoker_fang", inner.evoker_fang_entities.len()),
        ]
        .into_iter()
        .map(|(name, count)| (name.to_string(), count as u64))
        .collect()
    }
}
