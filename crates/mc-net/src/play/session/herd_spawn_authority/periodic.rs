use std::collections::{HashMap, HashSet};

use mc_entity::{SpawnEntity, Vec3};
use mc_physics::{Aabb, BlockMaterialIds};
use mc_world::{ChunkPos, WorldReadView};

use crate::play::spawn::chunk_pos_from_coords;
use crate::play::{HerdSpawn, is_hostile_entity, world_time_is_night};

use super::super::SessionRegistry;
use super::super::outbound::{VisibilityDispatch, dispatch_visibility_commands};
use super::commit::install_committed_herd_spawns_locked;
use super::planning::{
    MAX_NATURAL_TEMPLATES_PER_CHUNK, NaturalSpawnCapacities, plan_periodic_category,
};
use super::scheduler::{NaturalSpawnCategory, NaturalSpawnReport, NaturalSpawnScheduler};

#[derive(Clone, Copy)]
pub(crate) struct NaturalSpawnTickInput<'a> {
    pub(crate) tick: u64,
    pub(crate) friendly_interval: u64,
    pub(crate) hostile_interval: u64,
    pub(crate) simulation_distance: i32,
    pub(crate) world_read: Option<&'a WorldReadView>,
    pub(crate) materials: Option<&'a BlockMaterialIds>,
}

impl SessionRegistry {
    pub(in crate::play) fn register_natural_spawn_templates(
        &self,
        chunk: (i32, i32),
        templates: Vec<HerdSpawn>,
    ) -> bool {
        if templates.len() > MAX_NATURAL_TEMPLATES_PER_CHUNK
            || templates.iter().any(|template| template.chunk != chunk)
        {
            return false;
        }
        let mut inner = self.lock_inner("register natural spawn templates");
        if !inner.loaded_chunk_refcounts.contains_key(&chunk) {
            return false;
        }
        inner.natural_spawn_templates.insert(chunk, templates);
        true
    }

    pub(in crate::play) fn tick_periodic_natural_spawning(
        &self,
        scheduler: &mut NaturalSpawnScheduler,
        input: NaturalSpawnTickInput<'_>,
    ) -> (NaturalSpawnReport, Vec<VisibilityDispatch>) {
        let NaturalSpawnTickInput {
            tick,
            friendly_interval,
            hostile_interval,
            simulation_distance,
            world_read,
            materials,
        } = input;
        let friendly_due = friendly_interval != 0 && tick.is_multiple_of(friendly_interval);
        let hostile_due = hostile_interval != 0 && tick.is_multiple_of(hostile_interval);
        if !friendly_due && !hostile_due {
            return (NaturalSpawnReport::default(), Vec::new());
        }
        let mut report = NaturalSpawnReport::default();
        report.friendly.attempts = u64::from(friendly_due);
        report.hostile.attempts = u64::from(hostile_due);

        let active_chunks = self.simulation_inputs.active_chunks();
        let player_positions = {
            let inner = self.lock_inner("snapshot periodic natural spawning");
            inner
                .sessions
                .iter()
                .filter(|(id, _)| {
                    !inner.dead_sessions.contains(id)
                        && !inner.spectator_sessions.contains(id)
                        && !inner.client_unloaded_sessions.contains(id)
                })
                .map(|(_, session)| Vec3::new(session.pose.x, session.pose.y, session.pose.z))
                .collect::<Vec<_>>()
        };
        if player_positions.is_empty() {
            scheduler.record(tick, report);
            return (report, Vec::new());
        }

        let simulation_distance =
            simulation_distance.clamp(crate::MIN_VIEW_DISTANCE, crate::MAX_VIEW_DISTANCE) as u32;
        let player_chunks = player_positions
            .iter()
            .map(|position| chunk_pos_from_coords(position.x, position.z))
            .collect::<Vec<_>>();
        let mut friendly_chunks = if friendly_due {
            scheduler.select_chunks(NaturalSpawnCategory::Friendly, &active_chunks)
        } else {
            Vec::new()
        };
        let mut hostile_chunks = if hostile_due {
            scheduler.select_chunks(NaturalSpawnCategory::Hostile, &active_chunks)
        } else {
            Vec::new()
        };
        let within_simulation_distance = |chunk: &(i32, i32)| {
            player_chunks.iter().any(|player| {
                chunk.0.abs_diff(player.0) <= simulation_distance
                    && chunk.1.abs_diff(player.1) <= simulation_distance
            })
        };
        friendly_chunks.retain(&within_simulation_distance);
        hostile_chunks.retain(within_simulation_distance);
        let selected_chunks = friendly_chunks
            .iter()
            .chain(&hostile_chunks)
            .copied()
            .collect::<HashSet<_>>();
        if selected_chunks.is_empty() {
            scheduler.record(tick, report);
            return (report, Vec::new());
        }

        let (templates, natural_hostiles, natural_ground, natural_aquatic) = {
            let inner = self.lock_inner("snapshot selected periodic natural spawning");
            let templates = selected_chunks
                .iter()
                .filter_map(|chunk| {
                    inner
                        .natural_spawn_templates
                        .get(chunk)
                        .map(|templates| (*chunk, templates.clone()))
                })
                .collect::<HashMap<_, _>>();
            (
                templates,
                inner.natural_hostile_mobs.len(),
                inner.natural_ground_mobs.len(),
                inner.natural_aquatic_mobs.len(),
            )
        };
        let collision_chunks = selected_chunks
            .iter()
            .flat_map(|&(x, z)| {
                (-1..=1).flat_map(move |dx| (-1..=1).map(move |dz| (x + dx, z + dz)))
            })
            .collect::<HashSet<_>>();
        let active_entity_ids = self
            .simulation_inputs
            .entity_candidates_in_chunks(&collision_chunks);
        let projections = self
            .lock_entities("project periodic natural spawn collisions")
            .simulation_projections_for_ids(&active_entity_ids);
        let mut capacities =
            NaturalSpawnCapacities::from_counts(natural_hostiles, natural_ground, natural_aquatic);

        let chunk_positions = selected_chunks
            .iter()
            .map(|&(x, z)| ChunkPos { x, z })
            .collect::<Vec<_>>();
        let world_snapshot = world_read.map(|world| world.snapshot_chunks(&chunk_positions));
        let nighttime = world_time_is_night(self.world_time());
        let mob_behaviors = self.mob_behavior_table();
        let mut planned = Vec::<SpawnEntity>::new();
        let mut accepted_boxes = Vec::<(Vec3, Aabb)>::new();

        if friendly_due {
            let (category_report, category_candidates) = plan_periodic_category(
                NaturalSpawnCategory::Friendly,
                &friendly_chunks,
                &templates,
                world_snapshot.as_ref(),
                materials,
                &player_positions,
                &projections,
                &mut accepted_boxes,
                &mut capacities,
                tick,
                nighttime,
                &mob_behaviors,
            );
            report.friendly.merge(category_report);
            planned.extend(category_candidates);
        }
        if hostile_due {
            let (category_report, category_candidates) = plan_periodic_category(
                NaturalSpawnCategory::Hostile,
                &hostile_chunks,
                &templates,
                world_snapshot.as_ref(),
                materials,
                &player_positions,
                &projections,
                &mut accepted_boxes,
                &mut capacities,
                tick,
                nighttime,
                &mob_behaviors,
            );
            report.hostile.merge(category_report);
            planned.extend(category_candidates);
        }

        if planned.is_empty() {
            scheduler.record(tick, report);
            return (report, Vec::new());
        }
        let active_before_commit = self.simulation_inputs.active_chunks();
        planned.retain(|candidate| {
            active_before_commit.contains(&chunk_pos_from_coords(
                candidate.position.x,
                candidate.position.z,
            ))
        });
        let planned_friendly = planned
            .iter()
            .filter(|candidate| !is_hostile_entity(&candidate.type_name))
            .count();
        let planned_hostile = planned.len().saturating_sub(planned_friendly);
        if planned.is_empty() {
            scheduler.record(tick, report);
            return (report, Vec::new());
        }

        let committed = match self.commit_unique_herd_candidates(planned) {
            Ok(committed) => committed,
            Err(()) => {
                report.friendly.rejected_duplicate_or_stale = report
                    .friendly
                    .rejected_duplicate_or_stale
                    .saturating_add(planned_friendly as u64);
                report.hostile.rejected_duplicate_or_stale = report
                    .hostile
                    .rejected_duplicate_or_stale
                    .saturating_add(planned_hostile as u64);
                scheduler.record(tick, report);
                return (report, Vec::new());
            }
        };
        let active_after_commit = self.simulation_inputs.active_chunks();
        let mut stable = Vec::with_capacity(committed.len());
        let mut entities = self.lock_entities("fence periodic natural spawn chunks");
        for entity in committed {
            let chunk = chunk_pos_from_coords(entity.position.x, entity.position.z);
            if active_after_commit.contains(&chunk) {
                stable.push(entity);
            } else {
                let _ = entities.remove_if_current(entity);
            }
        }
        drop(entities);

        let friendly_committed = stable
            .iter()
            .filter(|entity| !is_hostile_entity(&entity.type_name))
            .count() as u64;
        let hostile_committed = stable.len() as u64 - friendly_committed;
        report.friendly.committed = report.friendly.committed.saturating_add(friendly_committed);
        report.hostile.committed = report.hostile.committed.saturating_add(hostile_committed);
        report.friendly.rejected_duplicate_or_stale = report
            .friendly
            .rejected_duplicate_or_stale
            .saturating_add(planned_friendly.saturating_sub(friendly_committed as usize) as u64);
        report.hostile.rejected_duplicate_or_stale = report
            .hostile
            .rejected_duplicate_or_stale
            .saturating_add(planned_hostile.saturating_sub(hostile_committed as usize) as u64);

        let dispatches = if stable.is_empty() {
            Vec::new()
        } else {
            let mut inner = self.lock_inner("publish periodic natural spawns");
            install_committed_herd_spawns_locked(&mut inner, stable, tick)
        };
        scheduler.record(tick, report);
        (report, dispatches)
    }

    pub(crate) fn tick_and_dispatch_periodic_natural_spawning(
        &self,
        scheduler: &mut NaturalSpawnScheduler,
        input: NaturalSpawnTickInput<'_>,
    ) -> NaturalSpawnReport {
        let (report, dispatches) = self.tick_periodic_natural_spawning(scheduler, input);
        dispatch_visibility_commands(dispatches);
        report
    }
}
