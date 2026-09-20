use super::*;

pub(super) fn prepare_entity_physics_inputs(
    config: &ServerConfig,
    world_read: Option<&mc_world::WorldReadView>,
    queries: &[play::EntityPhysicsQuery],
) -> Vec<EntityPhysicsInput> {
    let Some(world_read) = world_read else {
        return Vec::new();
    };
    if queries.is_empty() {
        return Vec::new();
    }
    let materials = cached_material_ids(config);
    let plans = entity_physics_sample_plans(queries);
    #[cfg(feature = "load-bench")]
    log_physics_sample_reuse(&plans);
    let chunk_positions = entity_physics_chunk_positions(&plans);
    let world_snapshot = world_read.snapshot_chunks(&chunk_positions);
    let chunks = chunk_positions
        .into_iter()
        .map(|position| (position, world_snapshot.chunk(position)))
        .collect();
    let snapshot = Arc::new(EntityPhysicsSnapshot {
        chunks,
        materials,
        blocks: Some(Arc::clone(&config.blocks)),
        powder_snow_states: powder_snow_state_ids(&config.blocks),
        collision_direct_lookup_compatible: cached_collision_direct_lookup_compatible(config),
    });
    entity_physics_inputs_from_snapshot(plans, snapshot)
}

#[cfg(feature = "load-bench")]
pub(super) fn log_physics_sample_reuse(plans: &[EntityPhysicsSamplePlan]) {
    if !tracing::enabled!(target: "mc_net::server", tracing::Level::INFO) {
        return;
    }
    if plans.len() <= ENTITY_PHYSICS_INLINE_LIMIT {
        return;
    }
    const WORDS_PER_COLUMN: usize = (MAX_Y - MIN_Y) as usize / 64;
    const CHUNK_WORDS: usize = mc_world::SECTION_DIM * mc_world::SECTION_DIM * WORDS_PER_COLUMN;
    let mut bound_cell_volume: u64 = 0;
    let mut chunk_cells: HashMap<mc_world::ChunkPos, Box<[u64; CHUNK_WORDS]>> = HashMap::new();
    for plan in plans {
        let bounds = plan.bounds;
        let min_y = bounds.min_y.max(MIN_Y);
        let max_y = bounds.max_y.min(MAX_Y - 1);
        if max_y < min_y {
            continue;
        }
        bound_cell_volume = bound_cell_volume.saturating_add(
            (i64::from(bounds.max_x) - i64::from(bounds.min_x) + 1)
                .saturating_mul(i64::from(max_y) - i64::from(min_y) + 1)
                .saturating_mul(i64::from(bounds.max_z) - i64::from(bounds.min_z) + 1)
                .max(0) as u64,
        );
        let mut y_mask = [0u64; WORDS_PER_COLUMN];
        for y in min_y..=max_y {
            let bit = (y - MIN_Y) as usize;
            y_mask[bit / 64] |= 1 << (bit % 64);
        }
        let section = mc_world::SECTION_DIM as i32;
        for x in bounds.min_x..=bounds.max_x {
            for z in bounds.min_z..=bounds.max_z {
                let cpos = mc_world::ChunkPos {
                    x: x.div_euclid(section),
                    z: z.div_euclid(section),
                };
                let bits = chunk_cells
                    .entry(cpos)
                    .or_insert_with(|| Box::new([0; CHUNK_WORDS]));
                let column = x.rem_euclid(section) as usize * mc_world::SECTION_DIM
                    + z.rem_euclid(section) as usize;
                let base = column * WORDS_PER_COLUMN;
                for (word, mask) in y_mask.iter().enumerate() {
                    bits[base + word] |= mask;
                }
            }
        }
    }
    let unique_sample_cells = chunk_cells
        .values()
        .flat_map(|bits| bits.iter())
        .map(|word| u64::from(word.count_ones()))
        .sum::<u64>();
    info!(
        queries = plans.len(),
        bound_cell_volume,
        unique_sample_cells,
        unique_chunks = chunk_cells.len(),
        "PHYSICS_SAMPLE_REUSE"
    );
}

pub(super) struct CompletedEntityPhysics {
    pub(super) tick: u64,
    pub(super) expected: Vec<play::EntityPhysicsQuery>,
    pub(super) owner_fence: Option<mc_entity::VersionedEntityKinematics>,
    pub(super) snapshot: Arc<EntityPhysicsSnapshot>,
    pub(super) steps: Vec<play::EntityPhysicsStep>,
    pub(super) projectile_physics_facts: play::EntityProjectilePhysicsFacts,
}

pub(super) fn spawn_entity_physics_job(
    tick: u64,
    expected: Vec<play::EntityPhysicsQuery>,
    owner_fence: Option<mc_entity::VersionedEntityKinematics>,
    cpu_resources: ChunkPipelineResources,
    inputs: Vec<EntityPhysicsInput>,
) -> tokio::task::JoinHandle<CompletedEntityPhysics> {
    debug_assert!(inputs.len() > ENTITY_PHYSICS_INLINE_LIMIT);
    let snapshot = Arc::clone(&inputs.first().expect("large physics batch").snapshot);
    let prepare_task = cpu_resources.begin_prepare_task();
    tokio::spawn(async move {
        let _prepare_task = prepare_task;
        let steps = step_entity_physics_inputs(cpu_resources, inputs).await;
        let projectile_physics_facts =
            entity_projectile_physics_facts_from_steps(tick, &expected, &snapshot, &steps);
        CompletedEntityPhysics {
            tick,
            expected,
            owner_fence,
            snapshot,
            steps,
            projectile_physics_facts,
        }
    })
}

pub(super) async fn wait_for_entity_physics_job(
    job: &mut Option<tokio::task::JoinHandle<CompletedEntityPhysics>>,
) -> Result<CompletedEntityPhysics, tokio::task::JoinError> {
    match job.as_mut() {
        Some(job) => job.await,
        None => std::future::pending().await,
    }
}

pub(super) async fn apply_entity_physics_job_result(
    result: Result<CompletedEntityPhysics, tokio::task::JoinError>,
    simulation_owner: &play::SimulationOwner,
    config: &ServerConfig,
    sessions: &play::SessionRegistry,
    cpu_resources: &ChunkPipelineResources,
    world_read: Option<&mc_world::WorldReadView>,
) {
    let completed = match result {
        Ok(completed) => completed,
        Err(error) if error.is_cancelled() => {
            debug!("entity physics job cancelled");
            return;
        }
        Err(error) => {
            warn!(%error, "entity physics job failed");
            return;
        }
    };
    let world_is_current = world_read
        .map(|world_read| entity_physics_snapshot_is_current(world_read, &completed.snapshot))
        .unwrap_or_else(|| completed.snapshot.chunks.is_empty());
    if !world_is_current {
        debug!(
            tick = completed.tick,
            entity_count = completed.expected.len(),
            "discarded entity physics result after world snapshot changed"
        );
        return;
    }
    let produced_steps = completed.steps.len();
    let accepted_steps = sessions.apply_entity_physics_if_current_and_dispatch_regional(
        cpu_resources,
        completed.tick,
        &completed.expected,
        &completed.steps,
        completed.owner_fence,
        &completed.projectile_physics_facts,
        None,
    );
    if accepted_steps.len() != produced_steps {
        debug!(
            tick = completed.tick,
            produced_steps,
            accepted_steps = accepted_steps.len(),
            "discarded stale entity physics results"
        );
    }
    let landed_falling_blocks =
        sessions.landed_falling_blocks(&completed.expected, &accepted_steps);
    if !landed_falling_blocks.is_empty() {
        simulation_owner
            .land_falling_blocks(config, sessions, world_read, &landed_falling_blocks)
            .await;
    }
}

pub(super) fn entity_physics_snapshot_is_current(
    world_read: &mc_world::WorldReadView,
    expected: &EntityPhysicsSnapshot,
) -> bool {
    let positions = expected.chunks.keys().copied().collect::<Vec<_>>();
    let current = world_read.snapshot_chunks(&positions);
    expected.chunks.iter().all(|(&position, expected_chunk)| {
        match (expected_chunk.as_ref(), current.chunk_ref(position)) {
            (Some(expected_chunk), Some(current_chunk)) => {
                Arc::ptr_eq(expected_chunk, current_chunk)
            }
            (None, None) => true,
            (Some(_), None) | (None, Some(_)) => false,
        }
    })
}

pub(super) async fn step_entity_physics_inputs(
    cpu_resources: ChunkPipelineResources,
    inputs: Vec<EntityPhysicsInput>,
) -> Vec<play::EntityPhysicsStep> {
    if inputs.is_empty() {
        return Vec::new();
    }

    if inputs.len() <= ENTITY_PHYSICS_INLINE_LIMIT {
        return inputs.into_iter().map(step_sampled_entity).collect();
    }

    let input_count = inputs.len();
    let workers = entity_physics_worker_count(&cpu_resources, input_count);
    let batch_size = input_count.div_ceil(workers);
    let mut batches = Vec::with_capacity(workers);
    let mut inputs = inputs.into_iter();
    for _ in 0..workers {
        let batch = inputs.by_ref().take(batch_size).collect::<Vec<_>>();
        if batch.is_empty() {
            break;
        }
        let permit = match cpu_resources.acquire_cpu().await {
            Ok(permit) => permit,
            Err(error) => {
                warn!(%error, "entity physics CPU admission closed");
                break;
            }
        };
        batches.push(tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _cpu = crate::resource_profile::CpuScope::new(
                crate::resource_profile::CpuStage::Simulation,
            );
            batch
                .into_iter()
                .map(step_sampled_entity)
                .collect::<Vec<_>>()
        }));
    }

    let mut steps = Vec::with_capacity(batches.len().saturating_mul(batch_size));
    for batch in batches {
        match batch.await {
            Ok(mut batch) => steps.append(&mut batch),
            Err(err) if err.is_cancelled() => debug!("entity physics worker cancelled"),
            Err(err) => warn!(error = %err, "entity physics worker failed"),
        }
    }

    steps
}

pub(super) const ENTITY_PHYSICS_INLINE_LIMIT: usize = 256;
pub(super) const ANIMAL_BREEDING_TICK_INTERVAL_TICKS: u16 = 20;

pub(super) fn entity_physics_worker_count(
    cpu_resources: &ChunkPipelineResources,
    input_count: usize,
) -> usize {
    // Common herd sizes finish inside one tick inline. Blocking workers are for
    // larger batches where their scheduling overhead is amortized.
    if input_count == 0 {
        return 0;
    }
    cpu_resources
        .cpu_capacity()
        .min(input_count.div_ceil(ENTITY_PHYSICS_INLINE_LIMIT))
        .max(1)
}

pub(super) struct EntityPhysicsInput {
    pub(super) query: play::EntityPhysicsQuery,
    pub(super) snapshot: Arc<EntityPhysicsSnapshot>,
    pub(super) complete_samples: bool,
}

pub(super) struct EntityPhysicsSamplePlan {
    pub(super) query: play::EntityPhysicsQuery,
    pub(super) bounds: EntityPhysicsSampleBounds,
}

pub(super) struct EntityPhysicsSnapshot {
    pub(super) chunks: HashMap<mc_world::ChunkPos, Option<mc_world::ChunkSnapshot>>,
    pub(super) materials: Arc<BlockMaterialIds>,
    pub(super) blocks: Option<Arc<BlockRegistry>>,
    /// Registry state ids of `minecraft:powder_snow`, precomputed once per
    /// snapshot: the vanilla collision table records powder snow as empty,
    /// so these states must always take the exact entity-dependent route.
    pub(super) powder_snow_states: Box<[u32]>,
    /// Proven once per registry at snapshot construction: when true, every
    /// state covered by the vanilla collision table resolves identically via
    /// direct `get(state_id)` and via the fingerprinted `get_for_state` route.
    pub(super) collision_direct_lookup_compatible: bool,
}

pub(super) struct SampledPhysicsWorld {
    pub(super) snapshot: Arc<EntityPhysicsSnapshot>,
    pub(super) entity_bottom: f64,
    pub(super) fall_distance: f64,
    pub(super) powder_snow_collision: PowderSnowCollision,
}

#[derive(Clone, Copy, Default)]
pub(super) enum PowderSnowCollision {
    #[default]
    None,
    WalkableMob,
    FallingBlock,
}

impl SampledPhysicsWorld {
    pub(super) fn for_query(
        snapshot: Arc<EntityPhysicsSnapshot>,
        query: play::EntityPhysicsQuery,
    ) -> Self {
        let powder_snow_collision = match query.kind {
            play::EntityPhysicsKind::PowderSnowWalkableLiving => PowderSnowCollision::WalkableMob,
            play::EntityPhysicsKind::FallingBlock => PowderSnowCollision::FallingBlock,
            _ => PowderSnowCollision::None,
        };
        Self {
            snapshot,
            entity_bottom: query.position.y,
            fall_distance: query.fall_distance,
            powder_snow_collision,
        }
    }

    pub(super) fn without_entity_context(snapshot: Arc<EntityPhysicsSnapshot>) -> Self {
        Self {
            snapshot,
            entity_bottom: f64::NEG_INFINITY,
            fall_distance: 0.0,
            powder_snow_collision: PowderSnowCollision::None,
        }
    }

    pub(super) fn state_id_at(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        if !(MIN_Y..MAX_Y).contains(&y) {
            return None;
        }
        let cpos = mc_world::ChunkPos {
            x: x.div_euclid(mc_world::SECTION_DIM as i32),
            z: z.div_euclid(mc_world::SECTION_DIM as i32),
        };
        let chunk = self.snapshot.chunks.get(&cpos).and_then(Option::as_ref)?;
        let local_x = x.rem_euclid(mc_world::SECTION_DIM as i32) as u8;
        let local_z = z.rem_euclid(mc_world::SECTION_DIM as i32) as u8;
        chunk.get_block(local_x, y, local_z).map(|state| state.0)
    }
}

pub(super) fn arrow_physics_facts_from_steps(
    tick: u64,
    expected: &[play::EntityPhysicsQuery],
    snapshot: &Arc<EntityPhysicsSnapshot>,
    steps: &[play::EntityPhysicsStep],
) -> Vec<play::ArrowPhysicsFact> {
    let sampler = SampledPhysicsWorld::without_entity_context(Arc::clone(snapshot));
    let mut expected = expected.iter();

    steps
        .iter()
        .filter_map(|step| {
            // Physics preserves query order and may omit rejected inputs. Walking
            // the ordered source once avoids an all-query index allocation.
            let query = expected.find(|query| query.id == step.id)?;
            let play::EntityPhysicsKind::ArrowProjectile { embedded_block, .. } = query.kind else {
                return None;
            };
            let endpoint_block =
                collision_block_touching_arrow_endpoint(&sampler, step.position, query.aabb);
            let block_hit = if query.velocity != mc_entity::Vec3::ZERO {
                endpoint_block.map(|(block_position, block_state)| play::ArrowBlockHitFact {
                    arrow_id: step.id,
                    block_state: mc_world::BlockStateId(block_state),
                    block_position,
                    // `step.position` is the contact endpoint resolved against this snapshot.
                    location: step.position,
                })
            } else {
                None
            };
            let retained_block_state = embedded_block.map(|position| {
                sampler
                    .state_id_at(position.x, position.y, position.z)
                    .unwrap_or(snapshot.materials.air)
            });
            let current_block_state = retained_block_state
                .or_else(|| endpoint_block.map(|(_, state)| state))
                .or_else(|| {
                    sampler.state_id_at(
                        step.position.x.floor() as i32,
                        step.position.y.floor() as i32,
                        step.position.z.floor() as i32,
                    )
                })
                .unwrap_or(snapshot.materials.air);
            let retained_supports_arrow = retained_block_state
                .is_some_and(|state| snapshot.materials.classify(state).is_solid());
            let embedded_in_block = if embedded_block.is_some() {
                retained_supports_arrow
            } else {
                endpoint_block.is_some()
            };
            let in_water = entity_bounds_overlap_water(&sampler, step.position, query.aabb);
            Some(play::ArrowPhysicsFact {
                arrow_id: step.id,
                block_hit,
                embedded_in_block,
                current_block_state: mc_world::BlockStateId(current_block_state),
                should_fall: !embedded_in_block,
                fall_velocity_scale: arrow_fall_velocity_scale(step.id, tick),
                in_water,
                // Weather is not yet a world authority; water is the complete
                // supported source for this combined vanilla predicate.
                in_water_or_rain: in_water,
            })
        })
        .collect()
}

pub(super) fn entity_projectile_physics_facts_from_steps(
    tick: u64,
    expected: &[play::EntityPhysicsQuery],
    snapshot: &Arc<EntityPhysicsSnapshot>,
    steps: &[play::EntityPhysicsStep],
) -> play::EntityProjectilePhysicsFacts {
    play::EntityProjectilePhysicsFacts {
        arrows: arrow_physics_facts_from_steps(tick, expected, snapshot, steps),
        hurting: hurting_projectile_physics_facts_from_steps(expected, snapshot, steps),
        throwable: throwable_projectile_physics_facts_from_steps(expected, snapshot, steps),
    }
}

pub(super) fn throwable_projectile_physics_facts_from_steps(
    expected: &[play::EntityPhysicsQuery],
    snapshot: &Arc<EntityPhysicsSnapshot>,
    steps: &[play::EntityPhysicsStep],
) -> Vec<play::HurtingProjectilePhysicsFact> {
    let sampler = SampledPhysicsWorld::without_entity_context(Arc::clone(snapshot));
    let mut expected = expected.iter();
    steps
        .iter()
        .filter_map(|step| {
            let query = expected.find(|query| query.id == step.id)?;
            if !matches!(
                query.kind,
                play::EntityPhysicsKind::ThrowableProjectile { .. }
            ) {
                return None;
            }
            let endpoint_block =
                collision_block_touching_arrow_endpoint(&sampler, step.position, query.aabb);
            let block_hit = endpoint_block.map(|(block_position, block_state)| {
                play::HurtingProjectileBlockHitFact {
                    projectile_id: step.id,
                    block_state: mc_world::BlockStateId(block_state),
                    block_position,
                    location: step.position,
                }
            });
            Some(play::HurtingProjectilePhysicsFact {
                projectile_id: step.id,
                block_hit,
                in_water: entity_bounds_overlap_water(&sampler, query.position, query.aabb),
            })
        })
        .collect()
}

pub(super) fn hurting_projectile_physics_facts_from_steps(
    expected: &[play::EntityPhysicsQuery],
    snapshot: &Arc<EntityPhysicsSnapshot>,
    steps: &[play::EntityPhysicsStep],
) -> Vec<play::HurtingProjectilePhysicsFact> {
    let sampler = SampledPhysicsWorld::without_entity_context(Arc::clone(snapshot));
    let mut expected = expected.iter();
    steps
        .iter()
        .filter_map(|step| {
            let query = expected.find(|query| query.id == step.id)?;
            if !matches!(
                query.kind,
                play::EntityPhysicsKind::HurtingProjectile { .. }
                    | play::EntityPhysicsKind::ShulkerBullet { .. }
            ) {
                return None;
            }
            let endpoint_block =
                collision_block_touching_arrow_endpoint(&sampler, step.position, query.aabb);
            let block_hit = endpoint_block.map(|(block_position, block_state)| {
                play::HurtingProjectileBlockHitFact {
                    projectile_id: step.id,
                    block_state: mc_world::BlockStateId(block_state),
                    block_position,
                    location: step.position,
                }
            });
            Some(play::HurtingProjectilePhysicsFact {
                projectile_id: step.id,
                block_hit,
                in_water: entity_bounds_overlap_water(&sampler, query.position, query.aabb),
            })
        })
        .collect()
}

pub(super) fn entity_bounds_overlap_water(
    sampler: &SampledPhysicsWorld,
    position: mc_entity::Vec3,
    aabb: mc_physics::Aabb,
) -> bool {
    const BOUNDS_EPSILON: f64 = 1.0e-9;
    let min_x = (position.x - aabb.half_width + BOUNDS_EPSILON).floor() as i32;
    let max_x = (position.x + aabb.half_width - BOUNDS_EPSILON).floor() as i32;
    let min_y = (position.y + BOUNDS_EPSILON).floor() as i32;
    let max_y = (position.y + aabb.height - BOUNDS_EPSILON).floor() as i32;
    let min_z = (position.z - aabb.half_width + BOUNDS_EPSILON).floor() as i32;
    let max_z = (position.z + aabb.half_width - BOUNDS_EPSILON).floor() as i32;
    (min_y..=max_y).any(|y| {
        (min_z..=max_z)
            .any(|z| (min_x..=max_x).any(|x| sampler.material_at(x, y, z) == BlockMaterial::Water))
    })
}

pub(super) fn arrow_fall_velocity_scale(entity: mc_entity::EntityId, tick: u64) -> mc_entity::Vec3 {
    fn component(seed: u64) -> f64 {
        let mut value = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^= value >> 31;
        (value >> 11) as f64 * (0.2 / ((1_u64 << 53) as f64))
    }

    let seed = tick ^ (entity.0 as i64 as u64).rotate_left(32);
    mc_entity::Vec3::new(
        component(seed),
        component(seed.wrapping_add(1)),
        component(seed.wrapping_add(2)),
    )
}

pub(super) fn collision_block_touching_arrow_endpoint(
    sampler: &SampledPhysicsWorld,
    position: mc_entity::Vec3,
    aabb: mc_physics::Aabb,
) -> Option<(mc_entity::projectile_26_1_2::BlockPosition, u32)> {
    const CONTACT_EPSILON: f64 = 1.0e-9;
    let min_x = (position.x - aabb.half_width - CONTACT_EPSILON).floor() as i32;
    let max_x = (position.x + aabb.half_width + CONTACT_EPSILON).floor() as i32;
    let min_y = (position.y - CONTACT_EPSILON).floor() as i32;
    let max_y = (position.y + aabb.height + CONTACT_EPSILON).floor() as i32;
    let min_z = (position.z - aabb.half_width - CONTACT_EPSILON).floor() as i32;
    let max_z = (position.z + aabb.half_width + CONTACT_EPSILON).floor() as i32;
    let mut first_colliding_state = None;

    for y in min_y..=max_y {
        for z in min_z..=max_z {
            for x in min_x..=max_x {
                let Some(state) = sampler.state_id_at(x, y, z) else {
                    continue;
                };
                sampler.collision_boxes_at(x, y, z, &mut |collision_box| {
                    let [
                        box_min_x,
                        box_min_y,
                        box_min_z,
                        box_max_x,
                        box_max_y,
                        box_max_z,
                    ] = collision_box.as_blocks();
                    let touches = position.x + aabb.half_width + CONTACT_EPSILON
                        >= f64::from(x) + box_min_x
                        && position.x - aabb.half_width - CONTACT_EPSILON
                            <= f64::from(x) + box_max_x
                        && position.y + aabb.height + CONTACT_EPSILON >= f64::from(y) + box_min_y
                        && position.y - CONTACT_EPSILON <= f64::from(y) + box_max_y
                        && position.z + aabb.half_width + CONTACT_EPSILON
                            >= f64::from(z) + box_min_z
                        && position.z - aabb.half_width - CONTACT_EPSILON
                            <= f64::from(z) + box_max_z;
                    if touches {
                        let candidate = (x, y, z, state);
                        if first_colliding_state.is_none_or(|first| candidate < first) {
                            first_colliding_state = Some(candidate);
                        }
                    }
                });
            }
        }
    }
    first_colliding_state.map(|(x, y, z, state)| {
        (
            mc_entity::projectile_26_1_2::BlockPosition::new(x, y, z),
            state,
        )
    })
}

#[derive(Clone, Copy)]
pub(super) struct EntityPhysicsSampleBounds {
    pub(super) min_x: i32,
    pub(super) max_x: i32,
    pub(super) min_y: i32,
    pub(super) max_y: i32,
    pub(super) min_z: i32,
    pub(super) max_z: i32,
}

impl BlockSampler for SampledPhysicsWorld {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        self.state_id_at(x, y, z)
            .map_or(BlockMaterial::Air, |state| {
                self.snapshot.materials.classify(state)
            })
    }

    fn collision_height_at(&self, x: i32, y: i32, z: i32) -> Option<BlockCollisionHeight> {
        self.state_id_at(x, y, z)
            .and_then(|state| self.snapshot.materials.collision_height(state))
    }

    fn max_collision_box_y(&self) -> u8 {
        let max_y = mc_data::collision_shapes::vanilla_collision_shapes().max_box_y();
        u8::try_from((max_y + 255) / 256).expect("vanilla collision height fits u8")
    }

    fn collision_boxes_at(&self, x: i32, y: i32, z: i32, emit: &mut dyn FnMut(BlockCollisionBox)) {
        let Some(state) = self.state_id_at(x, y, z) else {
            return;
        };
        // Direct-lookup-compatible registries resolve the dominant vanilla
        // cells in O(1): empty shapes emit nothing and full-cube shapes are
        // exactly `BlockCollisionBox::FULL_BLOCK`, so the registry lookup,
        // string compare, and table decode below only run for Complex and
        // Missing states. Powder snow's vanilla shape is entity-dependent
        // (recorded empty), so its precomputed state ids divert it to the
        // exact entity-dependent route below.
        if self.snapshot.collision_direct_lookup_compatible
            && !self.snapshot.powder_snow_states.contains(&state)
        {
            match mc_data::collision_shapes::vanilla_collision_class(state) {
                mc_data::collision_shapes::CollisionClass::Empty => return,
                mc_data::collision_shapes::CollisionClass::FullCube => {
                    emit(BlockCollisionBox::FULL_BLOCK);
                    return;
                }
                mc_data::collision_shapes::CollisionClass::Complex
                | mc_data::collision_shapes::CollisionClass::Missing => {}
            }
        }
        let exact_shape = self
            .snapshot
            .blocks
            .as_ref()
            .and_then(|blocks| blocks.by_id(mc_world::BlockStateId(state)))
            .and_then(|block| {
                let table = mc_data::collision_shapes::vanilla_collision_shapes();
                let shape = if self.snapshot.collision_direct_lookup_compatible {
                    table.get(state)
                } else {
                    table.get_for_state(state, &block.block.id, &block.properties)
                };
                shape.map(|shape| (block.block.id.as_str(), shape))
            });
        if let Some(("minecraft:powder_snow", _)) = exact_shape {
            if self.fall_distance > 2.5 {
                if let Some(collision_box) = BlockCollisionBox::from_fixed_4096([
                    0,
                    0,
                    0,
                    4096,
                    (0.9_f32 * 4096.0) as i16,
                    4096,
                ]) {
                    emit(collision_box);
                }
                return;
            }
            match self.powder_snow_collision {
                PowderSnowCollision::FallingBlock => {
                    emit(BlockCollisionBox::FULL_BLOCK);
                    return;
                }
                PowderSnowCollision::WalkableMob
                    if self.entity_bottom > f64::from(y) + 1.0 - 1.0e-5_f32 as f64 =>
                {
                    emit(BlockCollisionBox::FULL_BLOCK);
                    return;
                }
                PowderSnowCollision::None | PowderSnowCollision::WalkableMob => {}
            }
        }
        if let Some((_, boxes)) = exact_shape {
            for collision_box in boxes.iter() {
                if let Some(collision_box) =
                    BlockCollisionBox::from_fixed_4096(collision_box.coordinates())
                {
                    emit(collision_box);
                }
            }
        } else if let Some(height) = self.snapshot.materials.collision_height(state) {
            let max_y = (height.as_blocks() * 16.0) as u8;
            if let Some(collision_box) = BlockCollisionBox::from_sixteenths(0, 0, 0, 16, max_y, 16)
            {
                emit(collision_box);
            }
        }
    }
}

#[cfg(test)]
pub(super) fn sample_entity_physics_input(
    query: play::EntityPhysicsQuery,
    storage: &mut WorldStorage,
    materials: &BlockMaterialIds,
) -> EntityPhysicsInput {
    let mut plans = entity_physics_sample_plans(&[query]);
    let chunks = entity_physics_chunk_snapshots(&plans, |cpos| storage.cached_chunk_snapshot(cpos));
    let registry = storage.registry_arc();
    let snapshot = Arc::new(EntityPhysicsSnapshot {
        chunks,
        materials: Arc::new(materials.clone()),
        powder_snow_states: powder_snow_state_ids(&registry),
        collision_direct_lookup_compatible: collision_direct_lookup_compatible(&registry),
        blocks: Some(registry),
    });
    entity_physics_inputs_from_snapshot(std::mem::take(&mut plans), snapshot)
        .pop()
        .expect("one query yields one physics input")
}

pub(super) fn entity_physics_sample_plans(
    queries: &[play::EntityPhysicsQuery],
) -> Vec<EntityPhysicsSamplePlan> {
    queries
        .iter()
        .copied()
        .map(|query| EntityPhysicsSamplePlan {
            query,
            bounds: entity_physics_sample_bounds(query),
        })
        .collect()
}

#[cfg(test)]
pub(super) fn entity_physics_chunk_snapshots(
    plans: &[EntityPhysicsSamplePlan],
    mut cached_chunk: impl FnMut(mc_world::ChunkPos) -> Option<mc_world::ChunkSnapshot>,
) -> HashMap<mc_world::ChunkPos, Option<mc_world::ChunkSnapshot>> {
    entity_physics_chunk_positions(plans)
        .into_iter()
        .map(|position| (position, cached_chunk(position)))
        .collect()
}

pub(super) fn entity_physics_chunk_positions(
    plans: &[EntityPhysicsSamplePlan],
) -> Vec<mc_world::ChunkPos> {
    let mut chunks = HashSet::new();
    for plan in plans {
        if plan.bounds.max_y < MIN_Y || plan.bounds.min_y >= MAX_Y {
            continue;
        }
        let min_chunk_x = plan.bounds.min_x.div_euclid(mc_world::SECTION_DIM as i32);
        let max_chunk_x = plan.bounds.max_x.div_euclid(mc_world::SECTION_DIM as i32);
        let min_chunk_z = plan.bounds.min_z.div_euclid(mc_world::SECTION_DIM as i32);
        let max_chunk_z = plan.bounds.max_z.div_euclid(mc_world::SECTION_DIM as i32);
        for x in min_chunk_x..=max_chunk_x {
            for z in min_chunk_z..=max_chunk_z {
                let cpos = mc_world::ChunkPos { x, z };
                chunks.insert(cpos);
            }
        }
    }
    chunks.into_iter().collect()
}

pub(super) fn entity_physics_inputs_from_snapshot(
    plans: Vec<EntityPhysicsSamplePlan>,
    snapshot: Arc<EntityPhysicsSnapshot>,
) -> Vec<EntityPhysicsInput> {
    plans
        .into_iter()
        .map(|plan| {
            let complete_samples = entity_physics_samples_are_complete(&plan, &snapshot.chunks);
            EntityPhysicsInput {
                query: plan.query,
                snapshot: Arc::clone(&snapshot),
                complete_samples,
            }
        })
        .collect()
}

pub(super) fn entity_physics_samples_are_complete(
    plan: &EntityPhysicsSamplePlan,
    chunks: &HashMap<mc_world::ChunkPos, Option<mc_world::ChunkSnapshot>>,
) -> bool {
    if plan.bounds.max_y < MIN_Y || plan.bounds.min_y >= MAX_Y {
        return true;
    }
    let min_chunk_x = plan.bounds.min_x.div_euclid(mc_world::SECTION_DIM as i32);
    let max_chunk_x = plan.bounds.max_x.div_euclid(mc_world::SECTION_DIM as i32);
    let min_chunk_z = plan.bounds.min_z.div_euclid(mc_world::SECTION_DIM as i32);
    let max_chunk_z = plan.bounds.max_z.div_euclid(mc_world::SECTION_DIM as i32);
    for x in min_chunk_x..=max_chunk_x {
        for z in min_chunk_z..=max_chunk_z {
            if !chunks
                .get(&mc_world::ChunkPos { x, z })
                .is_some_and(Option::is_some)
            {
                return false;
            }
        }
    }
    true
}

pub(super) fn entity_physics_bounds_velocity(query: play::EntityPhysicsQuery) -> mc_entity::Vec3 {
    let velocity = mc_entity::projectile_26_1_2::Vec3::new(
        query.velocity.x,
        query.velocity.y,
        query.velocity.z,
    );
    let next = match query.kind {
        play::EntityPhysicsKind::HurtingProjectile {
            acceleration_power_bits,
            ..
        } => mc_entity::projectile_26_1_2::next_hurting_projectile_velocity(
            velocity,
            f64::from_bits(acceleration_power_bits),
            false,
        )
        .ok(),
        play::EntityPhysicsKind::ThrowableProjectile { gravity_bits, .. } => {
            mc_entity::projectile_26_1_2::next_throwable_velocity(
                velocity,
                f64::from_bits(gravity_bits),
                false,
                false,
            )
        }
        _ => return query.velocity,
    };
    next.map(|velocity| mc_entity::Vec3::new(velocity.x, velocity.y, velocity.z))
        .unwrap_or(query.velocity)
}

pub(super) fn entity_physics_sample_bounds(
    query: play::EntityPhysicsQuery,
) -> EntityPhysicsSampleBounds {
    let config = physics_config_for_query(query);
    let body = EntityBody {
        position: physics_vec(query.position),
        velocity: physics_vec(entity_physics_bounds_velocity(query)),
        aabb: query.aabb,
        on_ground: query.on_ground,
    };
    let next_x = body.position.x + body.velocity.x * config.tick_seconds;
    let next_y = body.position.y + body.velocity.y * config.tick_seconds;
    let next_z = body.position.z + body.velocity.z * config.tick_seconds;
    let half = body.aabb.half_width;
    let min_x = (body.position.x.min(next_x) - half - 1.0).floor() as i32;
    let max_x = (body.position.x.max(next_x) + half + 1.0).floor() as i32;
    let min_z = (body.position.z.min(next_z) - half - 1.0).floor() as i32;
    let max_z = (body.position.z.max(next_z) + half + 1.0).floor() as i32;
    let min_y = (body.position.y.min(next_y) - 2.0).floor() as i32;
    let max_y = (body.position.y.max(next_y) + body.aabb.height + 2.0).floor() as i32;

    EntityPhysicsSampleBounds {
        min_x,
        max_x,
        min_y,
        max_y,
        min_z,
        max_z,
    }
}

pub(super) fn step_sampled_entity(input: EntityPhysicsInput) -> play::EntityPhysicsStep {
    if matches!(input.query.kind, play::EntityPhysicsKind::ExternalFlight) {
        return play::EntityPhysicsStep {
            id: input.query.id,
            position: input.query.position,
            velocity: input.query.velocity,
            on_ground: false,
            horizontal_collision: false,
        };
    }
    if !input.complete_samples {
        return play::EntityPhysicsStep {
            id: input.query.id,
            position: input.query.position,
            velocity: mc_entity::Vec3::ZERO,
            on_ground: input.query.on_ground,
            horizontal_collision: false,
        };
    }
    let sampler = SampledPhysicsWorld::for_query(input.snapshot, input.query);
    let physics_velocity = match input.query.kind {
        play::EntityPhysicsKind::Immobile => mc_entity::Vec3::ZERO,
        play::EntityPhysicsKind::HurtingProjectile {
            acceleration_power_bits,
            ..
        } => {
            let in_water =
                entity_bounds_overlap_water(&sampler, input.query.position, input.query.aabb);
            let velocity = mc_entity::projectile_26_1_2::Vec3::new(
                input.query.velocity.x,
                input.query.velocity.y,
                input.query.velocity.z,
            );
            mc_entity::projectile_26_1_2::next_hurting_projectile_velocity(
                velocity,
                f64::from_bits(acceleration_power_bits),
                in_water,
            )
            .map(|velocity| mc_entity::Vec3::new(velocity.x, velocity.y, velocity.z))
            .unwrap_or(input.query.velocity)
        }
        play::EntityPhysicsKind::ThrowableProjectile { gravity_bits, .. } => {
            let in_water =
                entity_bounds_overlap_water(&sampler, input.query.position, input.query.aabb);
            let velocity = mc_entity::projectile_26_1_2::Vec3::new(
                input.query.velocity.x,
                input.query.velocity.y,
                input.query.velocity.z,
            );
            mc_entity::projectile_26_1_2::next_throwable_velocity(
                velocity,
                f64::from_bits(gravity_bits),
                false,
                in_water,
            )
            .map(|velocity| mc_entity::Vec3::new(velocity.x, velocity.y, velocity.z))
            .unwrap_or(input.query.velocity)
        }
        _ => input.query.velocity,
    };
    let result = mc_physics::step_entity(
        EntityBody {
            position: physics_vec(input.query.position),
            velocity: physics_vec(physics_velocity),
            aabb: input.query.aabb,
            on_ground: input.query.on_ground,
        },
        &sampler,
        physics_config_for_query(input.query),
    );
    play::EntityPhysicsStep {
        id: input.query.id,
        position: entity_vec(result.body.position),
        velocity: entity_vec(result.body.velocity),
        on_ground: result.body.on_ground,
        horizontal_collision: result.horizontal_collision
            && matches!(
                input.query.kind,
                play::EntityPhysicsKind::Living
                    | play::EntityPhysicsKind::PowderSnowWalkableLiving
                    | play::EntityPhysicsKind::FishLiving
                    | play::EntityPhysicsKind::SquidLiving
                    | play::EntityPhysicsKind::AquaticLiving
            ),
    }
}

pub(super) fn physics_config_for_query(query: play::EntityPhysicsQuery) -> PhysicsConfig {
    match query.kind {
        play::EntityPhysicsKind::Default => PhysicsConfig::default(),
        play::EntityPhysicsKind::Item => PhysicsConfig {
            // Vanilla ItemEntity: 0.04 blocks/tick^2 gravity and 0.98
            // friction on every axis (16 b/s^2 and 0.98 here), while water
            // adds the fluid-push 0.8 damping plus a small lift. The lift is
            // tuned so the terminal rise is ~0.8 b/s: an item surfacing
            // exits the water cell at that speed and the resulting air hop
            // (v^2 / 2g ~= 0.02 blocks) stays sub-pixel, so items rest at
            // the surface instead of limit-cycling across the fluid cell
            // boundary the way the shared default config made them.
            gravity: 16.0,
            air_drag: 0.98,
            vertical_air_drag: 0.98,
            water_drag: 0.8,
            water_buoyancy: 4.0,
            step_height: 0.0,
            ..PhysicsConfig::default()
        },
        play::EntityPhysicsKind::Immobile | play::EntityPhysicsKind::ExternalFlight => {
            PhysicsConfig {
                gravity: 0.0,
                air_drag: 1.0,
                vertical_air_drag: 1.0,
                water_drag: 1.0,
                water_buoyancy: 0.0,
                step_height: 0.0,
                ..PhysicsConfig::default()
            }
        }
        play::EntityPhysicsKind::Living | play::EntityPhysicsKind::PowderSnowWalkableLiving => {
            PhysicsConfig::living_entity()
        }
        play::EntityPhysicsKind::AquaticLiving => PhysicsConfig::aquatic_entity(),
        play::EntityPhysicsKind::FishLiving => PhysicsConfig::fish_entity(),
        play::EntityPhysicsKind::SquidLiving => PhysicsConfig::squid_entity(),
        play::EntityPhysicsKind::FallingBlock => PhysicsConfig::default(),
        play::EntityPhysicsKind::ArrowProjectile { .. }
        | play::EntityPhysicsKind::ShulkerBullet { .. }
        | play::EntityPhysicsKind::HurtingProjectile { .. }
        | play::EntityPhysicsKind::ThrowableProjectile { .. } => {
            let mut config = PhysicsConfig::arrow_projectile();
            // Retained projectile velocity is blocks per Minecraft tick. This
            // adapter resolves only the authoritative collision endpoint; the
            // projectile kernels own inertia/drag/gravity ordering.
            config.tick_seconds = 1.0;
            config.gravity = 0.0;
            config.air_drag = 1.0;
            config.vertical_air_drag = 1.0;
            config.water_drag = 1.0;
            config
        }
    }
}

pub(super) fn physics_vec(vec: mc_entity::Vec3) -> mc_physics::Vec3 {
    mc_physics::Vec3::new(vec.x, vec.y, vec.z)
}

pub(super) fn entity_vec(vec: mc_physics::Vec3) -> mc_entity::Vec3 {
    mc_entity::Vec3::new(vec.x, vec.y, vec.z)
}

pub(super) fn cached_material_ids(config: &ServerConfig) -> Arc<BlockMaterialIds> {
    let key = (
        Arc::as_ptr(&config.blocks) as usize,
        Arc::as_ptr(&config.block_facts) as usize,
    );
    let cache_lock = PHYSICS_MATERIAL_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut cache =
        crate::lock_policy::lock_benign_mutex(cache_lock, "server.physics_material_cache");
    if let Some((blocks, facts, materials)) = cache.get(&key)
        && blocks
            .upgrade()
            .is_some_and(|blocks| Arc::ptr_eq(&blocks, &config.blocks))
        && facts
            .upgrade()
            .is_some_and(|facts| Arc::ptr_eq(&facts, &config.block_facts))
    {
        return Arc::clone(materials);
    }

    let materials = Arc::new(material_ids(&config.blocks, &config.block_facts));
    cache.insert(
        key,
        (
            Arc::downgrade(&config.blocks),
            Arc::downgrade(&config.block_facts),
            Arc::clone(&materials),
        ),
    );
    materials
}

/// Whether every state covered by the vanilla collision table resolves to the
/// same shape through the registry fingerprint route (`by_id` +
/// `get_for_state`) as through the direct state-id route (`get`). Proven once
/// per registry; the hot sampler path only reads the resulting bool.
pub(super) fn collision_direct_lookup_compatible(blocks: &BlockRegistry) -> bool {
    let table = mc_data::collision_shapes::vanilla_collision_shapes();
    (0..table.covered_state_count()).all(|state| {
        let state = u32::try_from(state).expect("covered state id fits u32");
        blocks
            .by_id(mc_world::BlockStateId(state))
            .is_some_and(|block| {
                table.get_for_state(state, &block.block.id, &block.properties) == table.get(state)
            })
    })
}

/// Snapshot-local registry state ids of `minecraft:powder_snow`, computed
/// once per snapshot so the hot sampler path never string-compares block
/// names per sampled cell.
pub(super) fn powder_snow_state_ids(blocks: &BlockRegistry) -> Box<[u32]> {
    let Some(block) =
        blocks.block(&Identifier::parse("minecraft:powder_snow").expect("static identifier"))
    else {
        return Box::default();
    };
    block.states.iter().map(|state| state.0).collect()
}

pub(super) fn cached_collision_direct_lookup_compatible(config: &ServerConfig) -> bool {
    let key = Arc::as_ptr(&config.blocks) as usize;
    let cache_lock =
        COLLISION_DIRECT_LOOKUP_CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
    let mut cache =
        crate::lock_policy::lock_benign_mutex(cache_lock, "server.collision_direct_lookup_cache");
    if let Some((blocks, compatible)) = cache.get(&key)
        && blocks
            .upgrade()
            .is_some_and(|blocks| Arc::ptr_eq(&blocks, &config.blocks))
    {
        return *compatible;
    }

    let compatible = collision_direct_lookup_compatible(&config.blocks);
    cache.insert(key, (Arc::downgrade(&config.blocks), compatible));
    compatible
}

pub(super) fn material_ids(blocks: &BlockRegistry, facts: &BlockFactsTable) -> BlockMaterialIds {
    let state = |name: &str| {
        blocks
            .block(&Identifier::parse(name).expect("static identifier"))
            .map(|block| block.default.0)
    };
    let passable = crate::play::passive_entity_passable_blocks(blocks)
        .into_iter()
        .map(|state| state.0)
        .collect();
    let farmland = blocks
        .block(&Identifier::parse("minecraft:farmland").expect("static identifier"))
        .map(|block| block.states.iter().map(|state| state.0).collect())
        .unwrap_or_default();

    BlockMaterialIds::new(
        state("minecraft:air").unwrap_or(0),
        state("minecraft:water"),
        state("minecraft:lava"),
    )
    .with_water_states(fluid_material_states(blocks, facts, FluidKind::Water))
    .with_lava_states(fluid_material_states(blocks, facts, FluidKind::Lava))
    .with_passable(passable)
    .with_collision_height(
        farmland,
        BlockCollisionHeight::from_sixteenths(15).expect("valid farmland collision height"),
    )
}

pub(super) fn fluid_material_states(
    blocks: &BlockRegistry,
    facts: &BlockFactsTable,
    kind: FluidKind,
) -> Vec<u32> {
    blocks
        .states()
        .filter(|state| {
            facts
                .fluid(state.id.0)
                .is_some_and(|fluid| fluid.kind == kind)
        })
        .map(|state| state.id.0)
        .collect()
}
