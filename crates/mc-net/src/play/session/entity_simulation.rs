use super::entity_lifecycle::{
    move_entity_chunk_locked, remove_server_entity_locked, track_entity_chunk_locked,
};
use super::interaction_geometry::{
    distance_sq, entity_aabb, entity_geometry, entity_is_near_player_chunk,
};
use super::visibility::ordered_session_recipient;
use super::*;

mod persistence_projection;

use persistence_projection::{
    EntityPersistenceMetadata, maximum_persisted_age, project_owner_save, restore_timing,
};

#[cfg(test)]
std::thread_local! {
    static MOVEMENT_VISIBILITY_INDEX_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static MOVEMENT_VISIBILITY_INDEX_EDGE_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static MOVEMENT_EXHAUSTIVE_MEMBERSHIP_CHECKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct MovementFanoutWork {
    pub(super) index_builds: usize,
    pub(super) index_edge_visits: usize,
    pub(super) exhaustive_membership_checks: usize,
}

#[cfg(test)]
pub(super) fn reset_movement_fanout_work() {
    MOVEMENT_VISIBILITY_INDEX_BUILDS.set(0);
    MOVEMENT_VISIBILITY_INDEX_EDGE_VISITS.set(0);
    MOVEMENT_EXHAUSTIVE_MEMBERSHIP_CHECKS.set(0);
}

#[cfg(test)]
pub(super) fn take_movement_fanout_work() -> MovementFanoutWork {
    MovementFanoutWork {
        index_builds: MOVEMENT_VISIBILITY_INDEX_BUILDS.replace(0),
        index_edge_visits: MOVEMENT_VISIBILITY_INDEX_EDGE_VISITS.replace(0),
        exhaustive_membership_checks: MOVEMENT_EXHAUSTIVE_MEMBERSHIP_CHECKS.replace(0),
    }
}

#[cfg(test)]
fn record_movement_visibility_index_build() {
    MOVEMENT_VISIBILITY_INDEX_BUILDS.set(MOVEMENT_VISIBILITY_INDEX_BUILDS.get() + 1);
}

#[cfg(test)]
fn record_movement_visibility_index_edge_visit() {
    MOVEMENT_VISIBILITY_INDEX_EDGE_VISITS.set(MOVEMENT_VISIBILITY_INDEX_EDGE_VISITS.get() + 1);
}

#[cfg(test)]
fn record_movement_exhaustive_membership_check() {
    MOVEMENT_EXHAUSTIVE_MEMBERSHIP_CHECKS.set(MOVEMENT_EXHAUSTIVE_MEMBERSHIP_CHECKS.get() + 1);
}

impl SessionRegistry {
    pub(in crate::play) fn tick_entities_and_collect_physics_queries_owned(
        &self,
        _authority: &SimulationAuthority,
        cpu_resources: &crate::chunk_pipeline::ChunkPipelineResources,
        tick: u64,
        pathing_candidates_per_entity: usize,
        simulation_distance: i32,
        pathing: Option<(&mc_world::WorldReadView, &mc_physics::BlockMaterialIds)>,
    ) -> Vec<EntityPhysicsQuery> {
        self.tick_entities_and_collect_physics_queries_core(
            Some(cpu_resources),
            tick,
            pathing_candidates_per_entity,
            simulation_distance,
            pathing,
        )
    }

    #[cfg(test)]
    pub(crate) fn tick_entities_and_collect_physics_queries(
        &self,
        tick: u64,
    ) -> Vec<EntityPhysicsQuery> {
        self.tick_entities_and_collect_physics_queries_core(
            None,
            tick,
            PathingBudget::DEFAULT.max_candidates_per_entity,
            DEFAULT_VIEW_DISTANCE,
            None,
        )
    }

    #[cfg(test)]
    pub(crate) fn tick_entities_and_collect_physics_queries_with_simulation_distance(
        &self,
        tick: u64,
        simulation_distance: i32,
    ) -> Vec<EntityPhysicsQuery> {
        self.tick_entities_and_collect_physics_queries_core(
            None,
            tick,
            PathingBudget::DEFAULT.max_candidates_per_entity,
            simulation_distance,
            None,
        )
    }

    #[cfg(test)]
    pub(crate) fn tick_entities_and_collect_physics_queries_with_pathing_budget(
        &self,
        tick: u64,
        pathing_candidates_per_entity: usize,
    ) -> Vec<EntityPhysicsQuery> {
        self.tick_entities_and_collect_physics_queries_core(
            None,
            tick,
            pathing_candidates_per_entity,
            DEFAULT_VIEW_DISTANCE,
            None,
        )
    }

    #[cfg(test)]
    pub(crate) fn tick_entities_and_collect_physics_queries_with_terrain(
        &self,
        tick: u64,
        world_read: &mc_world::WorldReadView,
        pathing_materials: &mc_physics::BlockMaterialIds,
    ) -> Vec<EntityPhysicsQuery> {
        self.tick_entities_and_collect_physics_queries_core(
            None,
            tick,
            PathingBudget::DEFAULT.max_candidates_per_entity,
            DEFAULT_VIEW_DISTANCE,
            Some((world_read, pathing_materials)),
        )
    }

    fn tick_entities_and_collect_physics_queries_core(
        &self,
        cpu_resources: Option<&crate::chunk_pipeline::ChunkPipelineResources>,
        tick: u64,
        pathing_candidates_per_entity: usize,
        simulation_distance: i32,
        pathing: Option<(&mc_world::WorldReadView, &mc_physics::BlockMaterialIds)>,
    ) -> Vec<EntityPhysicsQuery> {
        let (world_read, pathing_materials) = pathing.unzip();
        let (
            active_chunks,
            active_entity_candidates,
            player_positions,
            terrain_pathing_entities,
            sheep_grazing_entities,
            has_tracked_entities,
        ) = {
            let inner = self.lock_inner("snapshot entity tick inputs");
            let active_chunks = inner
                .loaded_chunk_refcounts
                .keys()
                .copied()
                .collect::<HashSet<_>>();
            let active_entity_candidates = active_chunks
                .iter()
                .filter_map(|chunk| inner.entities_by_chunk.get(chunk))
                .flat_map(|entities| entities.iter().copied())
                .collect::<HashSet<_>>();
            let player_positions = inner
                .sessions
                .values()
                .map(|session| Vec3::new(session.pose.x, session.pose.y, session.pose.z))
                .collect::<Vec<_>>();
            (
                active_chunks,
                active_entity_candidates,
                player_positions,
                inner.terrain_pathing_entities.clone(),
                inner
                    .sheep_grazing_ticks
                    .keys()
                    .copied()
                    .collect::<HashSet<_>>(),
                !inner.entity_chunks.is_empty(),
            )
        };
        if player_positions.is_empty() && !has_tracked_entities {
            return Vec::new();
        }
        let mut entities = self.lock_entities("prepare entity goals");
        if player_positions.is_empty() {
            update_hostile_targets(&mut entities, &player_positions, None);
            return Vec::new();
        }
        if active_chunks.is_empty() {
            return Vec::new();
        }
        let mut active_entity_ids = HashSet::new();
        let mut active_entity_aabbs = HashMap::new();
        let mut active_entity_kinds = HashMap::new();
        entities.visit_simulation_entities_for_ids(&active_entity_candidates, |entity| {
            #[cfg(test)]
            self.active_entity_selection_visits
                .fetch_add(1, Ordering::Relaxed);
            if entity.lifecycle == EntityLifecycle::Alive {
                let chunk = chunk_pos_from_coords(entity.position.x, entity.position.z);
                if active_chunks.contains(&chunk)
                    && entity_is_near_player_chunk(chunk, &player_positions, simulation_distance)
                {
                    active_entity_ids.insert(entity.id);
                    active_entity_aabbs.insert(
                        entity.id,
                        entity_geometry(entity.type_name, entity.animal).aabb,
                    );
                    active_entity_kinds.insert(
                        entity.id,
                        if entity.type_name == "minecraft:arrow" {
                            EntityPhysicsKind::ArrowProjectile
                        } else if entity.item_stack.is_none()
                            && entity.experience_value.is_none()
                            && entity.block_state.is_none()
                            && entity.vehicle.is_none()
                        {
                            EntityPhysicsKind::Living
                        } else {
                            EntityPhysicsKind::Default
                        },
                    );
                }
            }
        });
        if active_entity_ids.is_empty() {
            return Vec::new();
        }
        update_hostile_targets(&mut entities, &player_positions, Some(&active_entity_ids));
        let goal_entity_ids = active_entity_ids
            .difference(&sheep_grazing_entities)
            .copied()
            .collect::<HashSet<_>>();
        let unprojected_entity_ids = active_entity_ids
            .difference(&goal_entity_ids)
            .copied()
            .collect::<HashSet<_>>();
        let prepared_goal_tick =
            entities.prepare_goal_tick_with_pathing_for_ids(tick, &goal_entity_ids);
        drop(entities);
        #[cfg(test)]
        self.pause_before_entity_goal_compute_for_test();
        let goal_budget = PathingBudget {
            max_candidates_per_entity: pathing_candidates_per_entity.max(1),
            ..PathingBudget::DEFAULT
        };
        let terrain_snapshot = if terrain_pathing_entities.is_empty() {
            None
        } else {
            world_read.zip(pathing_materials).map(|(world_read, _)| {
                let mut chunks = HashSet::new();
                prepared_goal_tick.visit_pathing_probe_positions(
                    goal_budget,
                    |entity, position| {
                        insert_terrain_snapshot_chunks_for_probe_position(
                            &mut chunks,
                            entity,
                            position,
                            &terrain_pathing_entities,
                            &active_entity_aabbs,
                            &active_chunks,
                        );
                    },
                );
                let chunks = sorted_chunk_positions(chunks);
                world_read.snapshot_chunks(&chunks)
            })
        };
        let pathing_probe = LoadedChunkPathingProbe::new(
            &active_chunks,
            &terrain_pathing_entities,
            &active_entity_aabbs,
            terrain_snapshot
                .as_ref()
                .zip(pathing_materials)
                .map(|(snapshot, materials)| LoadedTerrainPathingProbe::new(snapshot, materials)),
        );
        let goal_cpu_permits = cpu_resources
            .map(|resources| {
                acquire_regional_worker_permits(
                    resources,
                    prepared_goal_tick.parallel_batch_count(),
                )
            })
            .unwrap_or_default();
        let worker_count = goal_cpu_permits.len() + 1;
        let resolved_goal_tick = if worker_count > 1 {
            prepared_goal_tick.resolve_parallel(&pathing_probe, goal_budget, worker_count)
        } else {
            prepared_goal_tick.resolve(&pathing_probe, goal_budget)
        };
        let resolved_direct_paths = pathing_probe
            .resolved_direct_paths
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut entities = self.lock_entities("apply entity goals");
        let applied = entities
            .apply_prepared_goal_tick_and_alive_kinematics(resolved_goal_tick, &goal_entity_ids);
        let current_ids = if applied.is_some() {
            for id in &unprojected_entity_ids {
                active_entity_aabbs.remove(id);
                active_entity_kinds.remove(id);
            }
            &unprojected_entity_ids
        } else {
            active_entity_aabbs.clear();
            active_entity_kinds.clear();
            &active_entity_ids
        };
        let mut kinematics = applied
            .map(|(_, kinematics)| kinematics)
            .unwrap_or_default();
        entities.visit_simulation_entities_for_ids(current_ids, |entity| {
            if entity.lifecycle != EntityLifecycle::Alive {
                return;
            }
            let chunk = chunk_pos_from_coords(entity.position.x, entity.position.z);
            if !active_chunks.contains(&chunk)
                || !entity_is_near_player_chunk(chunk, &player_positions, simulation_distance)
            {
                return;
            }
            active_entity_aabbs.insert(
                entity.id,
                entity_geometry(entity.type_name, entity.animal).aabb,
            );
            active_entity_kinds.insert(
                entity.id,
                if entity.type_name == "minecraft:arrow" {
                    EntityPhysicsKind::ArrowProjectile
                } else if entity.item_stack.is_none()
                    && entity.experience_value.is_none()
                    && entity.block_state.is_none()
                    && entity.vehicle.is_none()
                {
                    EntityPhysicsKind::Living
                } else {
                    EntityPhysicsKind::Default
                },
            );
            kinematics.push(EntityKinematics {
                id: entity.id,
                position: entity.position,
                rotation: entity.rotation,
                velocity: entity.velocity,
                on_ground: entity.on_ground,
            });
        });
        kinematics.sort_unstable_by_key(|state| state.id);
        drop(goal_cpu_permits);
        let queries = kinematics
            .into_iter()
            .map(|state| EntityPhysicsQuery {
                id: state.id,
                position: state.position,
                velocity: state.velocity,
                aabb: active_entity_aabbs[&state.id],
                on_ground: state.on_ground,
                kind: active_entity_kinds[&state.id],
            })
            .collect();
        drop(entities);
        if !resolved_direct_paths.is_empty() {
            let mut inner = self.lock_inner("resolve direct entity pathing");
            for entity_id in resolved_direct_paths {
                inner.terrain_pathing_entities.remove(&entity_id);
            }
        }
        queries
    }

    pub(in crate::play) fn restore_persisted_entities_owned(
        &self,
        _authority: &SimulationAuthority,
        entities: impl IntoIterator<Item = impl Into<PersistedEntityRecord>>,
    ) -> usize {
        self.restore_persisted_entities_core(entities)
    }

    #[cfg(test)]
    pub(crate) fn restore_persisted_entities(
        &self,
        entities: impl IntoIterator<Item = impl Into<PersistedEntityRecord>>,
    ) -> usize {
        self.restore_persisted_entities_core(entities)
    }

    fn restore_persisted_entities_core(
        &self,
        entities: impl IntoIterator<Item = impl Into<PersistedEntityRecord>>,
    ) -> usize {
        let mut inner = self.lock_session_entities("restore persisted entities");
        let records = entities
            .into_iter()
            .map(Into::into)
            .collect::<Vec<PersistedEntityRecord>>();
        let max_age = maximum_persisted_age(&records);
        self.entity_lifecycle_tick
            .fetch_max(max_age, Ordering::AcqRel);
        inner.entity_lifecycle_tick = self.simulation_tick();
        let restored_timing = restore_timing(&records, inner.entity_lifecycle_tick);
        if !inner.entities.insert_authoritative_snapshots_batch(
            records.iter().map(|record| record.snapshot.clone()),
        ) {
            return 0;
        }
        let restored = records.len();
        for (record, timing) in records.into_iter().zip(restored_timing) {
            let entity = record.snapshot;
            let aabb = entity_aabb(&entity.type_name);
            let published = server_entity_snapshot_from(entity.clone());
            let type_id = entity.type_id;
            let entity_id = entity.id;
            let position = entity.position;
            inner
                .published_entity_snapshots
                .insert(entity_id, published);
            inner.entity_type_aabbs.entry(type_id).or_insert(aabb);
            track_entity_chunk_locked(&mut inner, entity_id, position);
            initialize_entity_wire_state_locked(&mut inner, entity_id);
            debug_assert_eq!(timing.entity_id, entity_id);
            inner
                .entity_spawn_ticks
                .insert(entity_id, timing.spawn_tick);
            if let Some(spawn_tick) = timing.item_spawn_tick {
                inner.item_spawn_ticks.insert(entity_id, spawn_tick);
            }
            if let Some(ready_tick) = timing.item_pickup_ready_tick {
                inner.item_pickup_ready_ticks.insert(entity_id, ready_tick);
            }
            if let Some(spawn_tick) = timing.arrow_spawn_tick {
                inner.arrow_spawn_ticks.insert(entity_id, spawn_tick);
            }
        }
        restored
    }

    #[cfg(test)]
    pub(crate) fn persisted_entity_records(&self) -> Vec<PersistedEntityRecord> {
        self.persisted_entity_save_snapshot().0
    }

    pub(crate) fn persisted_entity_save_snapshot(
        &self,
    ) -> (Vec<PersistedEntityRecord>, Vec<mc_entity::RegionPhase>) {
        let metadata = {
            let inner = self.lock_inner("snapshot persisted entity metadata");
            EntityPersistenceMetadata {
                lifecycle_tick: self.simulation_tick(),
                spawn_ticks: inner.entity_spawn_ticks.clone(),
                item_pickup_ready_ticks: inner.item_pickup_ready_ticks.clone(),
            }
        };
        #[cfg(test)]
        self.pause_before_entity_save_owner_barrier_for_test();
        let saved = owner_result(self.entities.handle.save_barrier());
        project_owner_save(saved, &metadata)
    }

    #[cfg(test)]
    pub(in crate::play) fn apply_entity_physics_and_dispatch_owned(
        &self,
        _authority: &SimulationAuthority,
        tick: u64,
        steps: &[EntityPhysicsStep],
    ) {
        let _ = self.apply_entity_physics_and_dispatch_core(None, tick, None, steps);
    }

    pub(in crate::play) fn apply_entity_physics_if_current_and_dispatch_owned(
        &self,
        _authority: &SimulationAuthority,
        cpu_resources: &crate::chunk_pipeline::ChunkPipelineResources,
        tick: u64,
        expected: &[EntityPhysicsQuery],
        steps: &[EntityPhysicsStep],
    ) -> Vec<EntityPhysicsStep> {
        self.apply_entity_physics_and_dispatch_core(
            Some(cpu_resources),
            tick,
            Some(expected),
            steps,
        )
    }

    #[cfg(test)]
    pub(in crate::play) fn compare_entity_shadow_owned(
        &self,
        _authority: &SimulationAuthority,
        tick: u64,
        stage: ShadowStage,
    ) -> Option<ShadowDivergence> {
        let mut entities = self.lock_entities("compare entity ECS shadow");
        let already_recorded = entities
            .shadow_comparison_stats()
            .first_divergence
            .is_some();
        match entities.compare_shadow(tick, stage) {
            Err(divergence) if !already_recorded => Some(*divergence),
            Ok(_) | Err(_) => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn apply_entity_physics_and_dispatch(&self, tick: u64, steps: &[EntityPhysicsStep]) {
        let _ = self.apply_entity_physics_and_dispatch_core(None, tick, None, steps);
    }

    #[cfg(test)]
    pub(crate) fn apply_entity_physics_if_current_and_dispatch(
        &self,
        tick: u64,
        expected: &[EntityPhysicsQuery],
        steps: &[EntityPhysicsStep],
    ) {
        let _ = self.apply_entity_physics_and_dispatch_core(None, tick, Some(expected), steps);
    }

    pub(super) fn apply_entity_physics_and_dispatch_core(
        &self,
        cpu_resources: Option<&crate::chunk_pipeline::ChunkPipelineResources>,
        tick: u64,
        expected: Option<&[EntityPhysicsQuery]>,
        steps: &[EntityPhysicsStep],
    ) -> Vec<EntityPhysicsStep> {
        let step_ids = steps.iter().map(|step| step.id).collect::<HashSet<_>>();
        let entities = self.lock_entities("prepare entity physics");
        entities.prefetch(&step_ids);
        let session_inner = self.lock_inner("apply entity physics");
        let mut inner = SessionEntityGuards {
            inner: session_inner,
            entities,
            entity_lifecycle_tick: self.simulation_tick(),
        };
        self.entity_lifecycle_tick.fetch_max(tick, Ordering::AcqRel);
        inner.entity_lifecycle_tick = self.simulation_tick();
        if inner.published_entity_snapshots.is_empty() {
            return Vec::new();
        }
        let expected_by_id = expected.map(|queries| {
            queries
                .iter()
                .map(|query| (query.id, *query))
                .collect::<HashMap<_, _>>()
        });
        let needs_filter = steps.iter().any(|step| {
            if !step.position.is_finite() || !step.velocity.is_finite() {
                return true;
            }
            let Some(expected_by_id) = expected_by_id.as_ref() else {
                return false;
            };
            let Some(expected) = expected_by_id.get(&step.id) else {
                return true;
            };
            !inner.entities.motion_state(step.id).is_some_and(|current| {
                current.position == expected.position
                    && current.velocity == expected.velocity
                    && current.on_ground == expected.on_ground
            })
        });
        let filtered_steps = needs_filter.then(|| {
            steps
                .iter()
                .copied()
                .filter(|step| {
                    if !step.position.is_finite() || !step.velocity.is_finite() {
                        return false;
                    }
                    let Some(expected_by_id) = expected_by_id.as_ref() else {
                        return true;
                    };
                    let Some(expected) = expected_by_id.get(&step.id) else {
                        return false;
                    };
                    inner.entities.motion_state(step.id).is_some_and(|current| {
                        current.position == expected.position
                            && current.velocity == expected.velocity
                            && current.on_ground == expected.on_ground
                    })
                })
                .collect::<Vec<_>>()
        });
        let steps = filtered_steps.as_deref().unwrap_or(steps);
        let mut dispatches = despawn_expired_arrows_locked(&mut inner);
        dispatches.extend(despawn_expired_items_locked(&mut inner));
        let old_chunks: HashMap<_, _> = steps
            .iter()
            .filter_map(|step| {
                inner
                    .entity_chunks
                    .get(&step.id)
                    .copied()
                    .map(|chunk| (step.id, chunk))
            })
            .collect();
        let old_publication: HashMap<_, _> = steps
            .iter()
            .filter_map(|step| {
                inner
                    .published_entity_snapshots
                    .get(&step.id)
                    .map(|snapshot| {
                        (
                            step.id,
                            (
                                snapshot.position,
                                snapshot.rotation,
                                snapshot.velocity,
                                snapshot.on_ground,
                            ),
                        )
                    })
            })
            .collect();
        let old_motion: HashMap<_, _> = steps
            .iter()
            .filter_map(|step| {
                let current = inner.entities.motion_state(step.id)?;
                inner
                    .published_entity_snapshots
                    .get(&step.id)
                    .map(|snapshot| {
                        (
                            step.id,
                            EntityMotionState {
                                id: step.id,
                                position: snapshot.position,
                                rotation: current.rotation,
                                velocity: snapshot.velocity,
                                on_ground: snapshot.on_ground,
                                is_item: current.is_item,
                                is_experience: current.is_experience,
                                is_arrow: current.is_arrow,
                                sends_velocity: current.sends_velocity,
                            },
                        )
                    })
            })
            .collect();
        let kinematics = steps
            .iter()
            .filter_map(|step| {
                old_motion.get(&step.id).map(|motion| {
                    let rotation = expected_by_id
                        .as_ref()
                        .and_then(|expected| expected.get(&step.id))
                        .filter(|expected| expected.kind == EntityPhysicsKind::Living)
                        .filter(|_| step.velocity.x.hypot(step.velocity.z) > 0.01)
                        .map_or(motion.rotation, |_| {
                            let yaw =
                                step.velocity.z.atan2(step.velocity.x).to_degrees() as f32 - 90.0;
                            Rotation {
                                yaw,
                                pitch: motion.rotation.pitch,
                                head_yaw: yaw,
                            }
                        });
                    EntityKinematics {
                        id: step.id,
                        position: step.position,
                        rotation,
                        velocity: step.velocity,
                        on_ground: step.on_ground,
                    }
                })
            })
            .collect::<Vec<_>>();
        let regional_batch_count = inner.entities.parallel_kinematics_batch_count(&kinematics);
        let regional_worker_permits = cpu_resources
            .map(|resources| acquire_regional_worker_permits(resources, regional_batch_count))
            .unwrap_or_default();
        let entity_lifecycle_tick = inner.entity_lifecycle_tick;
        let SessionEntityGuards {
            inner: session_inner,
            mut entities,
            ..
        } = inner;
        drop(session_inner);
        #[cfg(test)]
        self.pause_before_physics_owner_apply_for_test();
        let applied_kinematics = if regional_worker_permits.is_empty() {
            entities.apply_kinematics_authoritative(kinematics)
        } else {
            entities.apply_kinematics_parallel_authoritative(
                kinematics,
                regional_worker_permits.len() + 1,
            )
        };
        let applied_rotations = applied_kinematics
            .iter()
            .map(|state| (state.id, state.rotation))
            .collect::<HashMap<_, _>>();
        drop(regional_worker_permits);
        for id in &step_ids {
            entities.invalidate(*id);
        }
        entities.prefetch(&step_ids);
        let session_inner = self.lock_inner("publish entity physics");
        let mut inner = SessionEntityGuards {
            inner: session_inner,
            entities,
            entity_lifecycle_tick,
        };
        let input_steps = steps
            .iter()
            .map(|step| (step.id, *step))
            .collect::<HashMap<_, _>>();
        let applied_steps = applied_kinematics
            .into_iter()
            .filter(|state| {
                let publication_is_current = old_publication
                    .get(&state.id)
                    .zip(inner.published_entity_snapshots.get(&state.id))
                    .is_some_and(|(old, current)| {
                        *old == (
                            current.position,
                            current.rotation,
                            current.velocity,
                            current.on_ground,
                        )
                    });
                publication_is_current
                    && inner
                        .entities
                        .motion_state(state.id)
                        .is_some_and(|current| {
                            current.position == state.position
                                && current.rotation == state.rotation
                                && current.velocity == state.velocity
                                && current.on_ground == state.on_ground
                        })
            })
            .filter_map(|state| {
                let input = input_steps.get(&state.id)?;
                Some(EntityPhysicsStep {
                    id: state.id,
                    position: state.position,
                    velocity: state.velocity,
                    on_ground: state.on_ground,
                    horizontal_collision: input.horizontal_collision,
                })
            })
            .collect::<Vec<_>>();
        let steps = applied_steps.as_slice();
        for step in steps {
            if step.horizontal_collision && step.velocity.y <= 0.0 {
                inner.terrain_pathing_entities.insert(step.id);
            }
        }
        let chunk_crossings = steps
            .iter()
            .filter_map(|step| {
                let old_chunk = old_chunks.get(&step.id).copied()?;
                let new_chunk = chunk_pos_from_coords(step.position.x, step.position.z);
                (old_chunk != new_chunk).then_some((step.id, old_chunk, new_chunk))
            })
            .collect::<Vec<_>>();
        let old_observers_by_entity = chunk_crossings
            .iter()
            .map(|&(entity_id, _, _)| {
                #[cfg(test)]
                self.physics_boundary_observer_scans
                    .fetch_add(1, Ordering::Relaxed);
                (
                    entity_id,
                    visible_entity_observers_locked(&inner, entity_id)
                        .into_iter()
                        .collect::<HashSet<_>>(),
                )
            })
            .collect::<HashMap<_, _>>();
        for step in steps {
            let Some(motion) = old_motion.get(&step.id) else {
                continue;
            };
            if !inner.entities.contains(step.id) {
                continue;
            }
            let rotation = applied_rotations
                .get(&step.id)
                .copied()
                .unwrap_or(motion.rotation);
            if let Some(snapshot) = inner.published_entity_snapshots.get_mut(&step.id) {
                snapshot.position = step.position;
                snapshot.rotation = rotation;
                snapshot.velocity = step.velocity;
                snapshot.on_ground = step.on_ground;
            } else {
                let _ = publish_server_entity_snapshot_locked(&mut inner, step.id);
            }
        }
        for &(entity_id, old_chunk, new_chunk) in &chunk_crossings {
            move_entity_chunk_locked(&mut inner, entity_id, old_chunk, new_chunk);
        }
        dispatches.extend(resolve_arrow_entity_hits_locked(
            &mut inner,
            steps,
            &old_motion,
        ));
        for &(entity_id, old_chunk, new_chunk) in &chunk_crossings {
            dispatches.extend(refresh_entity_target_visibility_locked(
                &mut inner, entity_id, old_chunk, new_chunk,
            ));
        }
        let live_step_ids = if tick.is_multiple_of(ENTITY_MOVE_SEND_INTERVAL_TICKS) {
            let live_step_ids = steps
                .iter()
                .filter(|step| inner.entities.contains(step.id))
                .map(|step| step.id)
                .collect::<HashSet<_>>();
            for entity_id in &live_step_ids {
                if !inner.last_sent_entity_states.contains_key(entity_id) {
                    initialize_entity_wire_state_locked(&mut inner, *entity_id);
                }
            }
            live_step_ids
        } else {
            HashSet::new()
        };
        let lifecycle_tick = inner.entity_lifecycle_tick;
        let SessionEntityGuards {
            inner: session_inner,
            entities,
            ..
        } = inner;
        drop(entities);
        let mut inner = session_inner;
        #[cfg(test)]
        self.pause_before_session_movement_plan_for_test();
        let pickup_positions = steps
            .iter()
            .filter_map(|step| {
                let motion = old_motion.get(&step.id)?;
                (motion.is_experience
                    || (motion.is_item
                        && item_pickup_ready_locked(&inner, step.id, lifecycle_tick))
                    || (motion.is_arrow && step.on_ground && step.velocity == Vec3::ZERO))
                    .then_some(step.position)
            })
            .collect::<Vec<_>>();
        let mut pickup_sessions = if pickup_positions.is_empty() {
            Vec::new()
        } else {
            let radius_sq = ENTITY_PICKUP_RADIUS * ENTITY_PICKUP_RADIUS;
            inner
                .sessions
                .iter()
                .filter_map(|(&session_id, session)| {
                    let player = Vec3::new(session.pose.x, session.pose.y, session.pose.z);
                    pickup_positions
                        .iter()
                        .any(|position| distance_sq(*position, player) <= radius_sq)
                        .then_some(session_id)
                })
                .collect::<Vec<_>>()
        };
        pickup_sessions.extend(spawned_xp_observer_ids(&dispatches));
        if !tick.is_multiple_of(ENTITY_MOVE_SEND_INTERVAL_TICKS) {
            drop(inner);
            dispatches.extend(self.pickup_candidate_dispatches(pickup_sessions));
            dispatch_visibility_commands(dispatches);
            return steps.to_vec();
        }

        let mut movements = Vec::with_capacity(steps.len());
        for step in steps {
            let Some(motion) = old_motion.get(&step.id) else {
                continue;
            };
            if !live_step_ids.contains(&step.id) {
                continue;
            }
            let Some(last_sent) = inner.last_sent_entity_states.get(&step.id).copied() else {
                continue;
            };
            let delta = quantized_entity_delta(step.position, last_sent.position);
            let position_changed = delta != Vec3::ZERO;
            let rotation = applied_rotations
                .get(&step.id)
                .copied()
                .unwrap_or(motion.rotation);
            let rotation_changed = packed_rotation_changed(last_sent.rotation, rotation);
            let on_ground_changed = last_sent.on_ground != step.on_ground;
            let send_position_rotation = position_changed || rotation_changed || on_ground_changed;
            let send_velocity =
                motion.sends_velocity && entity_velocity_changed(last_sent.velocity, step.velocity);
            if !send_position_rotation && !send_velocity {
                continue;
            }
            let movement = ServerEntityMove {
                id: step.id,
                delta,
                velocity: step.velocity,
                rotation,
                on_ground: step.on_ground,
                send_position_rotation,
                send_velocity,
            };
            movements.push((step.id, movement));
            let sent = inner
                .last_sent_entity_states
                .get_mut(&step.id)
                .expect("entity wire state exists after lookup");
            if send_position_rotation {
                sent.position = step.position;
                sent.rotation = rotation;
                sent.on_ground = step.on_ground;
            }
            if send_velocity {
                sent.velocity = step.velocity;
            }
        }
        let mut movement_recipients = Vec::new();
        let mut current_observers_by_entity = None;
        if !movements.is_empty() {
            let session_count = inner.sessions.len();
            let visibility_edge_count = inner
                .sessions
                .values()
                .try_fold(0usize, |edge_count, observer| {
                    edge_count.checked_add(observer.visible_entities.len())
                });
            let estimated_exhaustive_cost = session_count.saturating_mul(movements.len());
            // Charge one extra unit per edge for reverse-map allocation and insertion.
            let use_reverse_index = visibility_edge_count
                .is_some_and(|edge_count| estimated_exhaustive_cost > edge_count.saturating_mul(2));

            movement_recipients.reserve(session_count);
            if use_reverse_index {
                #[cfg(test)]
                record_movement_visibility_index_build();
                let mut reverse_index = HashMap::<EntityId, Vec<usize>>::new();
                for (&observer_id, observer) in &inner.sessions {
                    let recipient_index = movement_recipients.len();
                    movement_recipients.push((
                        observer_id,
                        ordered_session_recipient(observer_id, observer),
                        None,
                    ));
                    for &entity_id in observer.visible_entities.iter() {
                        #[cfg(test)]
                        record_movement_visibility_index_edge_visit();
                        reverse_index
                            .entry(entity_id)
                            .or_default()
                            .push(recipient_index);
                    }
                }
                let indexed_visibility_edges = reverse_index
                    .values()
                    .try_fold(0usize, |edge_count, observer_indexes| {
                        edge_count.checked_add(observer_indexes.len())
                    });
                if indexed_visibility_edges == visibility_edge_count {
                    current_observers_by_entity = Some(reverse_index);
                } else {
                    for ((recipient_id, _, visible_entities), (&observer_id, observer)) in
                        movement_recipients.iter_mut().zip(&inner.sessions)
                    {
                        debug_assert_eq!(*recipient_id, observer_id);
                        *visible_entities = Some(Arc::clone(&observer.visible_entities));
                    }
                }
            } else {
                movement_recipients.extend(inner.sessions.iter().map(
                    |(&observer_id, observer)| {
                        (
                            observer_id,
                            ordered_session_recipient(observer_id, observer),
                            Some(Arc::clone(&observer.visible_entities)),
                        )
                    },
                ));
            }
        }
        drop(inner);

        dispatches.extend(self.pickup_candidate_dispatches(pickup_sessions));

        #[cfg(test)]
        self.pause_before_move_fanout_for_test();
        let mut movements_by_recipient = movement_recipients
            .into_iter()
            .map(|(observer_id, recipient, visible_entities)| {
                (observer_id, recipient, visible_entities, Vec::new())
            })
            .collect::<Vec<_>>();
        if let Some(current_observers_by_entity) = current_observers_by_entity.as_ref() {
            for (entity_id, movement) in &movements {
                let Some(candidate_indexes) = current_observers_by_entity.get(entity_id) else {
                    continue;
                };
                for &recipient_index in candidate_indexes {
                    let (observer_id, _, _, recipient_movements) =
                        &mut movements_by_recipient[recipient_index];
                    if old_observers_by_entity
                        .get(entity_id)
                        .is_none_or(|observers| observers.contains(observer_id))
                    {
                        recipient_movements.push(*movement);
                    }
                }
            }
        } else {
            for (observer_id, _, visible_entities, recipient_movements) in
                &mut movements_by_recipient
            {
                let visible_entities = visible_entities
                    .as_ref()
                    .expect("exhaustive movement fanout retains current visibility");
                recipient_movements.extend(movements.iter().filter_map(|(entity_id, movement)| {
                    #[cfg(test)]
                    record_movement_exhaustive_membership_check();
                    (visible_entities.contains(entity_id)
                        && old_observers_by_entity
                            .get(entity_id)
                            .is_none_or(|observers| observers.contains(observer_id)))
                    .then_some(*movement)
                }));
            }
        }
        let mut inner = self.lock_inner("order entity movement dispatches");
        let mut ordered_movements = Vec::with_capacity(movements_by_recipient.len());
        let mut canceled_recipients = Vec::new();
        let mut move_dispatch_count = 0usize;
        for (observer_id, recipient, _, mut movements) in movements_by_recipient {
            let Some(observer) = inner.sessions.get(&observer_id) else {
                canceled_recipients.push(recipient);
                continue;
            };
            movements.retain(|movement| observer.visible_entities.contains(&movement.id));
            if movements.is_empty() {
                canceled_recipients.push(recipient);
                continue;
            }
            move_dispatch_count += movements.len();
            ordered_movements.push((recipient, movements));
        }
        inner.entity_dispatches.move_relative += move_dispatch_count as u64;
        drop(inner);
        drop(canceled_recipients);

        dispatch_visibility_commands(dispatches);
        for (recipient, mut movements) in ordered_movements {
            let command = if movements.len() == 1 {
                OutboundCommand::MoveEntityRelative(movements.pop().expect("one movement"))
            } else {
                OutboundCommand::MoveEntitiesRelative(movements)
            };
            dispatch_visibility_command(&recipient, command);
        }
        steps.to_vec()
    }

    pub(crate) fn landed_falling_blocks(
        &self,
        steps: &[EntityPhysicsStep],
    ) -> Vec<LandedFallingBlock> {
        let entities = self.lock_entities("collect landed falling blocks");
        steps
            .iter()
            .filter(|step| step.on_ground)
            .filter_map(|step| {
                let entity = entities.snapshot(step.id)?;
                if entity.lifecycle != EntityLifecycle::Alive
                    || entity.type_name != "minecraft:falling_block"
                {
                    return None;
                }
                let state = entity.block_state?;
                Some(LandedFallingBlock {
                    id: step.id,
                    pos: mc_world::BlockPos {
                        x: step.position.x.floor() as i32,
                        y: step.position.y.floor() as i32,
                        z: step.position.z.floor() as i32,
                    },
                    state: mc_world::BlockStateId(state),
                })
            })
            .collect()
    }

    pub(crate) fn remove_landed_falling_blocks(&self, ids: &[EntityId]) {
        if ids.is_empty() {
            return;
        }
        let mut inner = self.lock_session_entities("remove landed falling blocks");
        let dispatches = ids
            .iter()
            .filter_map(|id| {
                remove_server_entity_locked(&mut inner, *id).map(|(_, dispatches)| dispatches)
            })
            .flatten()
            .collect::<Vec<_>>();
        drop(inner);
        dispatch_visibility_commands(dispatches);
    }
}
